// SPDX-License-Identifier: AGPL-3.0-only

use super::{Adb, CommandOutput, device_state_error, run_text_with_timeout};
use crate::{DeviceError, DeviceResult};
use std::time::{Duration, Instant};

pub const MAX_ADB_RECOVERY_STEPS: usize = 7;
pub const MAX_ADB_RECOVERY_TEXT_BYTES: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdbTransportState {
    Device,
    Offline,
    Unauthorized,
    Other,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdbRecoveryPhase {
    InitialState,
    InitialConnect,
    ConnectedState,
    Disconnect,
    Reconnect,
    Verify,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdbRecoveryPath {
    Connect,
    TargetDisconnectConnect,
}

#[derive(Clone, PartialEq, Eq)]
pub struct AdbRecoveryText {
    pub text: String,
    pub truncated: bool,
}
impl AdbRecoveryText {
    pub(crate) fn new(text: &str) -> Self {
        let mut end = text.len().min(MAX_ADB_RECOVERY_TEXT_BYTES);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        Self {
            text: text[..end].to_owned(),
            truncated: end < text.len(),
        }
    }
}
impl std::fmt::Debug for AdbRecoveryText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdbRecoveryText")
            .field("truncated", &self.truncated)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdbCommandEvidence {
    pub succeeded: bool,
    pub exit_code: Option<i32>,
    pub stdout: AdbRecoveryText,
    pub stderr: AdbRecoveryText,
    pub stdout_lossy_decode: bool,
    pub stderr_lossy_decode: bool,
    pub state: AdbTransportState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdbRecoveryStep {
    pub phase: AdbRecoveryPhase,
    pub attempt: u8,
    pub elapsed_ms: u64,
    pub command: Option<AdbCommandEvidence>,
    pub error: Option<AdbRecoveryText>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdbTargetRecovery {
    pub endpoint: AdbRecoveryText,
    pub initial_error: AdbRecoveryText,
    pub path: AdbRecoveryPath,
    pub budget_ms: u64,
    pub steps: Vec<AdbRecoveryStep>,
    pub final_state: AdbTransportState,
    pub recovered: bool,
    pub dropped_count: u16,
}

pub(crate) struct AdbInputReady {
    pub state: String,
    pub recovery: Option<AdbTargetRecovery>,
}

impl Adb {
    /// Called only by the fenced Runtime input factory; generic capture/connect
    /// callers continue to use ensure_device.
    pub(crate) fn ensure_input_device(
        &self,
        serial: &str,
        connect_allowed: bool,
    ) -> DeviceResult<AdbInputReady> {
        if !connect_allowed || !is_tcp_endpoint(serial) {
            return self
                .ensure_device(serial, connect_allowed)
                .map(|state| AdbInputReady {
                    state,
                    recovery: None,
                });
        }
        ensure_input_device_with_commands(serial, self.config.command_timeout, |args, remaining| {
            run_text_with_timeout(&self.config.adb_path, args, remaining)
        })
    }
}

pub(super) fn command_evidence(
    output: &CommandOutput,
    succeeded: bool,
    exit_code: Option<i32>,
) -> AdbCommandEvidence {
    let state = if output.stdout_lossy_decode || output.stderr_lossy_decode {
        AdbTransportState::Unknown
    } else if succeeded {
        match output.stdout.trim() {
            "device" => AdbTransportState::Device,
            "offline" => AdbTransportState::Offline,
            "unauthorized" => AdbTransportState::Unauthorized,
            _ => AdbTransportState::Other,
        }
    } else if output.stdout.trim().is_empty() {
        match output.stderr.trim().lines().next().unwrap_or_default() {
            "error: device offline" | "adb: error: device offline"
                if output.stderr.trim().lines().count() == 1 =>
            {
                AdbTransportState::Offline
            }
            "error: device unauthorized." | "error: device unauthorized" => {
                AdbTransportState::Unauthorized
            }
            _ => AdbTransportState::Unknown,
        }
    } else {
        AdbTransportState::Unknown
    };
    AdbCommandEvidence {
        succeeded,
        exit_code,
        stdout: AdbRecoveryText::new(&output.stdout),
        stderr: AdbRecoveryText::new(&output.stderr),
        stdout_lossy_decode: output.stdout_lossy_decode,
        stderr_lossy_decode: output.stderr_lossy_decode,
        state,
    }
}

fn observed_state(result: &DeviceResult<CommandOutput>) -> AdbTransportState {
    match result {
        Ok(output) => command_evidence(output, true, Some(0)).state,
        Err(error) => error
            .adb_command()
            .map_or(AdbTransportState::Unknown, |value| value.state),
    }
}

fn state_result(result: &DeviceResult<CommandOutput>) -> DeviceResult<String> {
    result
        .as_ref()
        .map(|output| output.stdout.trim().to_owned())
        .map_err(|error| (*error).clone())
}

pub(super) fn is_tcp_endpoint(serial: &str) -> bool {
    if serial.len() > 320 || serial.trim() != serial {
        return false;
    }
    let Some((host, port)) = serial.rsplit_once(':') else {
        return false;
    };
    if !port.bytes().all(|byte| byte.is_ascii_digit())
        || !matches!(port.parse::<u16>(), Ok(1..=u16::MAX))
    {
        return false;
    }
    if host.starts_with('[') && host.ends_with(']') {
        return host[1..host.len() - 1]
            .parse::<std::net::Ipv6Addr>()
            .is_ok();
    }
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

pub(super) fn ensure_input_device_with_commands(
    serial: &str,
    budget: Duration,
    mut run: impl FnMut(&[&str], Duration) -> DeviceResult<CommandOutput>,
) -> DeviceResult<AdbInputReady> {
    let started = Instant::now();
    let deadline = started
        .checked_add(budget)
        .ok_or_else(|| DeviceError::fatal("adb input connection deadline overflow"))?;
    let mut steps = Vec::with_capacity(MAX_ADB_RECOVERY_STEPS);
    let mut command = |phase, attempt, args: &[&str]| {
        let result = match deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
        {
            Some(remaining) => run(args, remaining),
            None => Err(DeviceError::fatal(
                "adb input connection deadline exhausted",
            )),
        };
        let evidence = match &result {
            Ok(output) => Some(command_evidence(output, true, Some(0))),
            Err(error) => error.adb_command().cloned(),
        };
        steps.push(AdbRecoveryStep {
            phase,
            attempt,
            elapsed_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
            command: evidence,
            error: result
                .as_ref()
                .err()
                .map(|error| AdbRecoveryText::new(error.message())),
        });
        result
    };
    let first = command(
        AdbRecoveryPhase::InitialState,
        1,
        &["-s", serial, "get-state"],
    );
    if observed_state(&first) == AdbTransportState::Device {
        return Ok(AdbInputReady {
            state: "device".to_owned(),
            recovery: None,
        });
    }
    let initial_error = match &first {
        Err(error) => AdbRecoveryText::new(error.message()),
        Ok(output) => {
            AdbRecoveryText::new(&format!("get-state returned {:?}", output.stdout.trim()))
        }
    };
    let connect = command(AdbRecoveryPhase::InitialConnect, 1, &["connect", serial]);
    let second = command(
        AdbRecoveryPhase::ConnectedState,
        1,
        &["-s", serial, "get-state"],
    );
    let cleanup_failure = [&first, &connect, &second].into_iter().find_map(|result| {
        result
            .as_ref()
            .err()
            .filter(|error| {
                error.resource_quiescence() == Some(crate::DeviceResourceQuiescence::Unconfirmed)
            })
            .cloned()
    });
    let primary = cleanup_failure.clone().unwrap_or_else(|| {
        device_state_error(
            serial,
            state_result(&second),
            Some(
                connect
                    .as_ref()
                    .map(|_| ())
                    .map_err(|error| (*error).clone()),
            ),
        )
    });
    let mut final_state = observed_state(&second);
    let mut path = AdbRecoveryPath::Connect;
    let mut recovery_error = None;
    if final_state == AdbTransportState::Offline
        && observed_state(&first) != AdbTransportState::Unauthorized
        && connect.is_ok()
        && cleanup_failure.is_none()
    {
        path = AdbRecoveryPath::TargetDisconnectConnect;
        let disconnect = command(AdbRecoveryPhase::Disconnect, 1, &["disconnect", serial]);
        match disconnect {
            Err(error) => recovery_error = Some(error),
            Ok(_) => match command(AdbRecoveryPhase::Reconnect, 1, &["connect", serial]) {
                Err(error) => recovery_error = Some(error),
                Ok(_) => {
                    for attempt in 1..=2 {
                        if attempt == 2 {
                            let remaining = deadline.saturating_duration_since(Instant::now());
                            std::thread::sleep(remaining.min(Duration::from_millis(100)));
                        }
                        let state = command(
                            AdbRecoveryPhase::Verify,
                            attempt,
                            &["-s", serial, "get-state"],
                        );
                        final_state = observed_state(&state);
                        if final_state != AdbTransportState::Offline || attempt == 2 {
                            if final_state != AdbTransportState::Device {
                                recovery_error =
                                    Some(device_state_error(serial, state_result(&state), None));
                            }
                            break;
                        }
                    }
                }
            },
        }
    }
    let recovered = final_state == AdbTransportState::Device
        && recovery_error.is_none()
        && cleanup_failure.is_none();
    let report = AdbTargetRecovery {
        endpoint: AdbRecoveryText::new(serial),
        initial_error,
        path,
        budget_ms: budget.as_millis().min(u128::from(u64::MAX)) as u64,
        steps,
        final_state,
        recovered,
        dropped_count: 0,
    };
    if recovered {
        Ok(AdbInputReady {
            state: "device".to_owned(),
            recovery: Some(report),
        })
    } else {
        let primary = if let Some(error) = recovery_error {
            let message = format!("{}; target recovery failed: {error}", primary.message());
            primary
                .with_severity_and_message(crate::DeviceErrorSeverity::Fatal, message)
                .merge_resource_cleanup(error)
        } else {
            primary
        };
        Err(primary.with_adb_recovery(report))
    }
}
