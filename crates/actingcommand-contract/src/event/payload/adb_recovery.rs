// SPDX-License-Identifier: AGPL-3.0-only

use super::{LifecycleNativeDetail, SanitizationError};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdbTransportState {
    Device,
    Offline,
    Unauthorized,
    Other,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdbRecoveryPhase {
    InitialState,
    InitialConnect,
    ConnectedState,
    Disconnect,
    Reconnect,
    Verify,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdbRecoveryPath {
    Connect,
    TargetDisconnectConnect,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdbCommandEvidence {
    pub succeeded: bool,
    pub exit_code: Option<i32>,
    pub stdout: Option<LifecycleNativeDetail>,
    pub stderr: Option<LifecycleNativeDetail>,
    pub stdout_lossy_decode: bool,
    pub stderr_lossy_decode: bool,
    pub state: AdbTransportState,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdbRecoveryStep {
    pub phase: AdbRecoveryPhase,
    pub attempt: u8,
    pub elapsed_ms: u64,
    pub command: Option<AdbCommandEvidence>,
    pub error: Option<LifecycleNativeDetail>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdbTargetRecovery {
    pub endpoint: LifecycleNativeDetail,
    pub initial_error: LifecycleNativeDetail,
    pub path: AdbRecoveryPath,
    pub budget_ms: u64,
    pub steps: Vec<AdbRecoveryStep>,
    pub final_state: AdbTransportState,
    pub recovered: bool,
    pub dropped_count: u16,
}
impl AdbTargetRecovery {
    pub(crate) fn validate(&self) -> Result<(), SanitizationError> {
        self.endpoint.validate()?;
        self.initial_error.validate()?;
        if self.steps.len() > 7
            || self.steps.is_empty()
            || (self.recovered && self.final_state != AdbTransportState::Device)
        {
            return Err(SanitizationError::new(
                "invalid_adb_target_recovery",
                "adb_recovery",
            ));
        }
        for step in &self.steps {
            if step.attempt == 0 || step.attempt > 2 {
                return Err(SanitizationError::new(
                    "invalid_adb_recovery_attempt",
                    "adb_recovery",
                ));
            }
            if let Some(error) = &step.error {
                error.validate()?;
            }
            if let Some(command) = &step.command {
                for detail in [&command.stdout, &command.stderr].into_iter().flatten() {
                    detail.validate()?;
                }
            }
        }
        Ok(())
    }
}
