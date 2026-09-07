// SPDX-License-Identifier: AGPL-3.0-only

use super::{CliResult, next_token, parse_token};
use actingcommand_ledger_forensics::{
    ForensicEventFilter, ForensicEventsRequest, ForensicOutput, ForensicRequest,
    MAX_FORENSIC_EVENTS,
};
use std::io::Write;
use std::path::PathBuf;

pub(super) const HELP: &str = "ledger --state-root <runtime-state> [--origin-module <module>] [--diagnostic-code <code>] [--severity <severity>] [--correlation-id <id>] [--after <sequence>] [--through <sequence>] [--limit <1..1024>]";

pub(super) fn run(tokens: &[String], output: &mut impl Write) -> CliResult<()> {
    if matches!(tokens, [flag] if flag == "--help" || flag == "-h") {
        writeln!(output, "{HELP}")?;
    } else {
        match actingcommand_ledger_forensics::run(parse_args(tokens)?)? {
            ForensicOutput::Machine(report) => {
                serde_json::to_writer(&mut *output, &report)?;
                output.write_all(b"\n")?;
            }
            ForensicOutput::Human(report) => {
                output.write_all(report.as_bytes())?;
                if !report.ends_with('\n') {
                    output.write_all(b"\n")?;
                }
            }
        }
    }
    output.flush()?;
    Ok(())
}

pub(super) fn parse_args(tokens: &[String]) -> CliResult<ForensicRequest> {
    let mut state_root = None;
    let mut origin_module = None;
    let mut diagnostic_code = None;
    let mut severity = None;
    let mut correlation_id = None;
    let mut after = None;
    let mut through = None;
    let mut limit = None;
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index].as_str() {
            "--state-root" if state_root.is_none() => {
                state_root = Some(PathBuf::from(next_token(
                    tokens,
                    &mut index,
                    "--state-root",
                )?));
            }
            "--origin-module" if origin_module.is_none() => {
                origin_module = Some(next_token(tokens, &mut index, "--origin-module")?);
            }
            "--diagnostic-code" if diagnostic_code.is_none() => {
                diagnostic_code = Some(next_token(tokens, &mut index, "--diagnostic-code")?);
            }
            "--severity" if severity.is_none() => {
                severity = Some(next_token(tokens, &mut index, "--severity")?);
            }
            "--correlation-id" if correlation_id.is_none() => {
                correlation_id = Some(next_token(tokens, &mut index, "--correlation-id")?);
            }
            "--after" if after.is_none() => {
                after = Some(parse_token(tokens, &mut index, "--after")?);
            }
            "--through" if through.is_none() => {
                through = Some(parse_token(tokens, &mut index, "--through")?);
            }
            "--limit" if limit.is_none() => {
                limit = Some(parse_token(tokens, &mut index, "--limit")?);
            }
            option => {
                return Err(format!("duplicate or unsupported ledger option: {option}").into());
            }
        }
    }
    let state_root = state_root.ok_or("ledger requires --state-root <runtime-state>")?;
    let filter =
        ForensicEventFilter::new(origin_module, diagnostic_code, severity, correlation_id)?;
    let events = ForensicEventsRequest::new(
        filter,
        after.unwrap_or(0),
        through,
        limit.unwrap_or(MAX_FORENSIC_EVENTS),
    )?;
    Ok(ForensicRequest::events(state_root, events))
}
