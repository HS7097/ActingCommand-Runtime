// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract as contract;
use actingcommand_device as device;

pub(crate) fn adb_recovery_record(
    value: &device::AdbTargetRecovery,
) -> contract::AdbTargetRecovery {
    contract::AdbTargetRecovery {
        endpoint: Some(detail(&value.endpoint)),
        initial_error: Some(detail(&value.initial_error)),
        path: match value.path {
            device::AdbRecoveryPath::Connect => contract::AdbRecoveryPath::Connect,
            device::AdbRecoveryPath::TargetDisconnectConnect => {
                contract::AdbRecoveryPath::TargetDisconnectConnect
            }
        },
        budget_ms: value.budget_ms,
        steps: value
            .steps
            .iter()
            .map(|step| contract::AdbRecoveryStep {
                phase: phase(step.phase),
                attempt: step.attempt,
                elapsed_ms: step.elapsed_ms,
                error: step.error.as_ref().map(detail),
                outcome: None,
                command: step
                    .command
                    .as_ref()
                    .map(|command| contract::AdbCommandEvidence {
                        succeeded: command.succeeded,
                        exit_code: command.exit_code,
                        stdout: (!command.stdout.text.is_empty()).then(|| detail(&command.stdout)),
                        stderr: (!command.stderr.text.is_empty()).then(|| detail(&command.stderr)),
                        stdout_lossy_decode: command.stdout_lossy_decode,
                        stderr_lossy_decode: command.stderr_lossy_decode,
                        state: state(command.state),
                        raw_text_redacted: command.raw_text_redacted,
                        outcome: None,
                    }),
            })
            .collect(),
        final_state: state(value.final_state),
        recovered: value.recovered,
        dropped_count: value.dropped_count,
        outcome: None,
    }
}
fn detail(value: &device::AdbRecoveryText) -> contract::LifecycleNativeDetail {
    contract::LifecycleNativeDetail::new(&value.text, value.truncated)
}

fn state(value: device::AdbTransportState) -> contract::AdbTransportState {
    match value {
        device::AdbTransportState::Device => contract::AdbTransportState::Device,
        device::AdbTransportState::Offline => contract::AdbTransportState::Offline,
        device::AdbTransportState::Unauthorized => contract::AdbTransportState::Unauthorized,
        device::AdbTransportState::Other => contract::AdbTransportState::Other,
        device::AdbTransportState::Unknown => contract::AdbTransportState::Unknown,
    }
}

fn phase(value: device::AdbRecoveryPhase) -> contract::AdbRecoveryPhase {
    match value {
        device::AdbRecoveryPhase::InitialState => contract::AdbRecoveryPhase::InitialState,
        device::AdbRecoveryPhase::InitialConnect => contract::AdbRecoveryPhase::InitialConnect,
        device::AdbRecoveryPhase::ConnectedState => contract::AdbRecoveryPhase::ConnectedState,
        device::AdbRecoveryPhase::Disconnect => contract::AdbRecoveryPhase::Disconnect,
        device::AdbRecoveryPhase::Reconnect => contract::AdbRecoveryPhase::Reconnect,
        device::AdbRecoveryPhase::Verify => contract::AdbRecoveryPhase::Verify,
    }
}
