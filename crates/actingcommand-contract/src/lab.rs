// SPDX-License-Identifier: AGPL-3.0-only

//! Stable protocol DTOs shared by the ActingLab application core and adapters.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

pub const CLI_SCHEMA_VERSION: &str = "0.2";

pub type LabResult<T> = Result<T, LabError>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope<T> {
    pub schema_version: String,
    pub cli_version: String,
    pub runtime_version: String,
    pub ok: bool,
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<EnvelopeError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<Value>,
}

impl<T> Envelope<T> {
    pub fn ok(
        schema_version: impl Into<String>,
        cli_version: impl Into<String>,
        runtime_version: impl Into<String>,
        command: impl Into<String>,
        data: T,
    ) -> Self {
        Self {
            schema_version: schema_version.into(),
            cli_version: cli_version.into(),
            runtime_version: runtime_version.into(),
            ok: true,
            command: command.into(),
            data: Some(data),
            error: None,
            run_id: None,
            artifacts: None,
        }
    }

    pub fn err(
        schema_version: impl Into<String>,
        cli_version: impl Into<String>,
        runtime_version: impl Into<String>,
        command: impl Into<String>,
        error: LabError,
    ) -> Self {
        Self {
            schema_version: schema_version.into(),
            cli_version: cli_version.into(),
            runtime_version: runtime_version.into(),
            ok: false,
            command: command.into(),
            data: None,
            error: Some(error.into()),
            run_id: None,
            artifacts: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvelopeError {
    pub code: String,
    pub message: String,
    pub blocked_by: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabErrorClass {
    UsageValidation,
    SafetyBlocked,
    DeviceInstance,
    RuntimeUnavailable,
    NotImplemented,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabError {
    pub class: LabErrorClass,
    pub code: String,
    pub message: String,
    pub blocked_by: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl LabError {
    pub fn usage(message: impl Into<String>) -> Self {
        Self::new(
            LabErrorClass::UsageValidation,
            "validation_failed",
            message,
            &[],
        )
    }

    pub fn package_invalid(message: impl Into<String>) -> Self {
        Self::new(
            LabErrorClass::UsageValidation,
            "package_invalid",
            message,
            &[],
        )
    }

    pub fn safety_blocked(
        code: impl Into<String>,
        message: impl Into<String>,
        blocked_by: &[&str],
    ) -> Self {
        Self::new(LabErrorClass::SafetyBlocked, code, message, blocked_by)
    }

    pub fn instance(message: impl Into<String>) -> Self {
        Self::new(
            LabErrorClass::DeviceInstance,
            "instance_not_found",
            message,
            &["instance"],
        )
    }

    pub fn device(message: impl Into<String>) -> Self {
        Self::new(
            LabErrorClass::DeviceInstance,
            "device_error",
            message,
            &["device"],
        )
    }

    pub fn runtime_not_running(message: impl Into<String>) -> Self {
        Self::new(
            LabErrorClass::RuntimeUnavailable,
            "runtime_not_running",
            message,
            &["running_runtime"],
        )
    }

    pub fn not_implemented(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(LabErrorClass::NotImplemented, code, message, &[])
    }

    pub fn new(
        class: LabErrorClass,
        code: impl Into<String>,
        message: impl Into<String>,
        blocked_by: &[&str],
    ) -> Self {
        Self {
            class,
            code: code.into(),
            message: message.into(),
            blocked_by: blocked_by.iter().map(|value| value.to_string()).collect(),
            details: None,
        }
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }
}

impl fmt::Display for LabError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for LabError {}

impl From<LabError> for EnvelopeError {
    fn from(error: LabError) -> Self {
        Self {
            code: error.code,
            message: error.message,
            blocked_by: error.blocked_by,
            details: error.details,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvDetected {
    pub key: String,
    pub value: String,
    pub confidence: f32,
    pub source: String,
    pub detector_id: String,
    pub detected_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvResolved {
    pub key: String,
    pub value: String,
    pub confidence: f32,
    pub source: String,
    pub detector_id: String,
    pub source_result: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NeedsDetection {
    pub status: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    pub detector_ids: Vec<String>,
    pub keys: Vec<EnvResolved>,
    pub recommended_action: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriveStage {
    Request,
    Recognition,
    EnvDetected,
    EnvResolved,
    EnvNeedsDetection,
    Planned,
    Executed,
    Finalizing,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriveRecord<T> {
    pub stage: DriveStage,
    pub command: String,
    pub req_id: String,
    pub payload: T,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LeaseGrant {
    pub schema_version: String,
    pub lease_id: String,
    pub req_id: String,
    pub instance: String,
    pub holder: String,
    pub holder_pid: u32,
    pub priority: String,
    pub acquired_at_ms: u64,
    pub updated_at_ms: u64,
    pub alive: bool,
    pub destructive_step_active: bool,
    pub preempt_requested: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArbitrationState {
    ReadonlyAccepted,
    LeaseGranted,
    Recovering,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArbitrationStatus {
    pub state: ArbitrationState,
    pub instance: String,
    pub req_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease: Option<LeaseGrant>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerProjection {
    pub written: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl LedgerProjection {
    pub fn written(path: impl Into<String>) -> Self {
        Self {
            written: true,
            path: Some(path.into()),
            reason: None,
        }
    }

    pub fn skipped(reason: impl Into<String>) -> Self {
        Self {
            written: false,
            path: None,
            reason: Some(reason.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn envelope_serialization_matches_current_success_shape() {
        let envelope = Envelope::ok("0.2", "0.1.0", "runtime", "recognize", json!({"x": 1}));
        assert_eq!(
            serde_json::to_value(envelope).expect("envelope"),
            json!({
                "schema_version": "0.2",
                "cli_version": "0.1.0",
                "runtime_version": "runtime",
                "ok": true,
                "command": "recognize",
                "data": {"x": 1}
            })
        );
    }

    #[test]
    fn semantic_error_does_not_contain_process_exit_code() {
        let error = LabError::safety_blocked(
            "target_not_visible",
            "target did not pass",
            &["visible_target"],
        );
        let value = serde_json::to_value(error).expect("error");
        assert!(value.get("exit_code").is_none());
        assert_eq!(value["code"], "target_not_visible");
    }

    #[test]
    fn drive_stage_uses_existing_ledger_spelling() {
        assert_eq!(
            serde_json::to_value(DriveStage::EnvNeedsDetection).expect("stage"),
            "env_needs_detection"
        );
    }
}
