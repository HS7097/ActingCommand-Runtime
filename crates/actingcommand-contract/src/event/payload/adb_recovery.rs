// SPDX-License-Identifier: AGPL-3.0-only

use super::{LifecycleNativeDetail, SanitizationError, is_false};
use crate::codes::SanitizationCode;
use crate::outcome::Outcome;
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
    /// Workflow #398 RD-1a: a device serial the Runtime holds a binding for was replaced by its
    /// label token `[device:<label>]` in `stdout` or `stderr`. Omitted when nothing was replaced.
    #[serde(default, skip_serializing_if = "is_false")]
    pub raw_text_redacted: bool,
    /// Workflow #383 register X13 (first writer B5, v0.12.1): the command's failure as an
    /// outcome, adb's text the `raw_text` of its `foreign_adb_error` link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<Outcome>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdbRecoveryStep {
    pub phase: AdbRecoveryPhase,
    pub attempt: u8,
    pub elapsed_ms: u64,
    pub command: Option<AdbCommandEvidence>,
    pub error: Option<LifecycleNativeDetail>,
    /// Register X13 (B5): the step's failure as an outcome.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<Outcome>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdbTargetRecovery {
    /// Register X13: optional, so B5 (v0.12.1) can move the device evidence to outcome links.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<LifecycleNativeDetail>,
    /// Register X13: optional, so B5 can carry the initial error as `outcome` instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_error: Option<LifecycleNativeDetail>,
    pub path: AdbRecoveryPath,
    pub budget_ms: u64,
    pub steps: Vec<AdbRecoveryStep>,
    pub final_state: AdbTransportState,
    pub recovered: bool,
    pub dropped_count: u16,
    /// Register X13 (B5): the error that started the recovery as an outcome, adb's text the
    /// `raw_text` of its `foreign_adb_error` link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<Outcome>,
}
impl AdbTargetRecovery {
    /// The highest sensitivity of the foreign text the evidence's outcomes carry (ledger
    /// revision 3, register E7: a record is at least as sensitive as its foreign text).
    pub fn foreign_text_sensitivity(&self) -> Option<crate::Sensitivity> {
        self.outcome
            .iter()
            .chain(self.steps.iter().flat_map(|step| {
                step.outcome.iter().chain(
                    step.command
                        .iter()
                        .flat_map(|command| command.outcome.iter()),
                )
            }))
            .filter_map(Outcome::foreign_text_sensitivity)
            .max()
    }

    pub(crate) fn validate(&self) -> Result<(), SanitizationError> {
        for detail in [&self.endpoint, &self.initial_error].into_iter().flatten() {
            detail.validate()?;
        }
        // Register X13: the error that started the recovery is recorded, as text or outcome.
        if self.steps.len() > 7
            || self.steps.is_empty()
            || (self.recovered && self.final_state != AdbTransportState::Device)
            || (self.initial_error.is_none() && self.outcome.is_none())
        {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidAdbTargetRecovery,
                "adb_recovery",
            ));
        }
        for step in &self.steps {
            if step.attempt == 0 || step.attempt > 2 {
                return Err(SanitizationError::new(
                    SanitizationCode::InvalidAdbRecoveryAttempt,
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
