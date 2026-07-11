// SPDX-License-Identifier: AGPL-3.0-only

//! Thin production CLI for correlation-scoped Runtime flows.

#![forbid(unsafe_code)]

use actingcommand_contract::{EventActor, EventSource};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig, RuntimeFlowOutput};
use std::env;
use std::ffi::OsString;
use std::fmt;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run(env::args_os().skip(1).collect()) {
        Ok(output) => match write_output(&output) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("FATAL actingctl: {error}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("FATAL actingctl: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: Vec<OsString>) -> Result<RuntimeFlowOutput, ActingctlError> {
    let invocation = Invocation::parse(arguments)?;
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        &invocation.state_root,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .map_err(ActingctlError::runtime)?;
    match invocation.command {
        Command::Reset => client
            .safe_reset(&invocation.instance)
            .map_err(ActingctlError::runtime),
        Command::Observe => client
            .observe_readonly(&invocation.instance)
            .map_err(ActingctlError::runtime),
    }
}

fn write_output(output: &RuntimeFlowOutput) -> Result<(), ActingctlError> {
    let stdout = io::stdout();
    let mut writer = stdout.lock();
    serde_json::to_writer(&mut writer, output).map_err(|_| ActingctlError::Output)?;
    writer.write_all(b"\n").map_err(|_| ActingctlError::Output)
}

struct Invocation {
    state_root: PathBuf,
    instance: String,
    command: Command,
}

enum Command {
    Observe,
    Reset,
}

impl Invocation {
    fn parse(arguments: Vec<OsString>) -> Result<Self, ActingctlError> {
        let Some(command) = arguments.first().and_then(|value| value.to_str()) else {
            return Err(ActingctlError::Usage);
        };
        let mut state_root = None;
        let mut instance = None;
        let mut index = 1;
        while index < arguments.len() {
            let flag = arguments[index].to_str().ok_or(ActingctlError::Usage)?;
            match flag {
                "--state-root" => {
                    state_root = Some(PathBuf::from(require_value(&arguments, &mut index)?));
                }
                "--instance" => {
                    instance = Some(require_text(&arguments, &mut index)?);
                }
                _ => return Err(ActingctlError::Usage),
            }
            index += 1;
        }
        let state_root = state_root.ok_or(ActingctlError::Usage)?;
        let instance = instance
            .filter(|value: &String| !value.trim().is_empty())
            .ok_or(ActingctlError::Usage)?;
        let command = match command {
            "reset" => Command::Reset,
            "observe" => Command::Observe,
            _ => return Err(ActingctlError::Usage),
        };
        Ok(Self {
            state_root,
            instance,
            command,
        })
    }
}

fn require_value(arguments: &[OsString], index: &mut usize) -> Result<OsString, ActingctlError> {
    *index += 1;
    arguments.get(*index).cloned().ok_or(ActingctlError::Usage)
}

fn require_text(arguments: &[OsString], index: &mut usize) -> Result<String, ActingctlError> {
    require_value(arguments, index)?
        .into_string()
        .map_err(|_| ActingctlError::Usage)
}

#[derive(Debug)]
enum ActingctlError {
    Usage,
    Runtime(actingcommand_runtime_client::RuntimeClientError),
    Output,
}

impl ActingctlError {
    fn runtime(error: actingcommand_runtime_client::RuntimeClientError) -> Self {
        Self::Runtime(error)
    }
}

impl fmt::Display for ActingctlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage => formatter
                .write_str("usage: actingctl <observe|reset> --state-root <path> --instance <id>"),
            Self::Runtime(error) => error.fmt(formatter),
            Self::Output => formatter.write_str("failed to write JSON output"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observe_uses_only_runtime_identity() {
        let args = ["observe", "--state-root", "state", "--instance", "ak.cn"]
            .into_iter()
            .map(OsString::from)
            .collect();
        assert!(Invocation::parse(args).is_ok());
    }

    #[test]
    fn client_commands_reject_capture_configuration() {
        let args = [
            "observe",
            "--state-root",
            "state",
            "--instance",
            "ak.cn",
            "--serial",
            "127.0.0.1:16416",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        assert!(Invocation::parse(args).is_err());
    }
}
