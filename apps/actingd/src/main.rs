// SPDX-License-Identifier: AGPL-3.0-only

//! Thin process adapter for the resident ActingCommand Runtime.

#![forbid(unsafe_code)]

mod config;

use actingcommand_runtime_host::{RuntimeHost, RuntimeHostError};
use config::RuntimeAssembly;
use std::env;
use std::error::Error;
use std::fmt;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(100);

fn main() -> ExitCode {
    match run(env::args_os().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("FATAL actingd: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: Vec<std::ffi::OsString>) -> Result<(), ActingdError> {
    let config_path = parse_arguments(arguments)?;
    let RuntimeAssembly { host, registry } = config::load(&config_path)
        .and_then(config::ActingdConfigFile::assemble)
        .map_err(ActingdError::config)?;
    let host = RuntimeHost::start(host, Arc::new(registry)).map_err(ActingdError::runtime)?;
    println!(
        "actingd ready pid={} host={} port={}",
        host.runtime_info().pid(),
        host.runtime_info().host(),
        host.runtime_info().port()
    );
    monitor(host)
}

fn monitor(host: RuntimeHost) -> Result<(), ActingdError> {
    loop {
        thread::sleep(HEALTH_POLL_INTERVAL);
        match host.fatal_error().map_err(ActingdError::runtime)? {
            Some(error) => {
                let close_error = host.close().err();
                return Err(
                    close_error.map_or_else(|| ActingdError::runtime(error), ActingdError::runtime)
                );
            }
            None => continue,
        }
    }
}

fn parse_arguments(arguments: Vec<std::ffi::OsString>) -> Result<PathBuf, ActingdError> {
    let [flag, path] = arguments.as_slice() else {
        return Err(ActingdError::config("usage_invalid"));
    };
    if flag != "--config" || path.is_empty() {
        return Err(ActingdError::config("usage_invalid"));
    }
    Ok(PathBuf::from(path))
}

#[derive(Debug)]
struct ActingdError {
    code: &'static str,
    runtime: Option<RuntimeHostError>,
}

impl ActingdError {
    const fn config(code: &'static str) -> Self {
        Self {
            code,
            runtime: None,
        }
    }

    fn runtime(error: RuntimeHostError) -> Self {
        Self {
            code: error.code(),
            runtime: Some(error),
        }
    }
}

impl fmt::Display for ActingdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.runtime {
            Some(error) => error.fmt(formatter),
            None => formatter.write_str(self.code),
        }
    }
}

impl Error for ActingdError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn process_adapter_requires_exact_config_argument() {
        assert!(parse_arguments(Vec::new()).is_err());
        assert!(parse_arguments(vec![OsString::from("--config")]).is_err());
        assert!(
            parse_arguments(vec![
                OsString::from("--config"),
                OsString::from("actingd.json")
            ])
            .is_ok()
        );
    }
}
