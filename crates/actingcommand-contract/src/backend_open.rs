// SPDX-License-Identifier: AGPL-3.0-only

use crate::{LifecycleNativeDetail, SanitizationError};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendOpenEntry {
    Input,
    Capture,
    NemuPair,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendObservationStatus {
    Passed,
    Failed,
    #[default]
    Unknown,
    SimulationNotApplicable,
}

impl BackendObservationStatus {
    fn is_unknown(&self) -> bool {
        *self == Self::Unknown
    }
}

/// Parameters actually checked by one original native connection attempt.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendInputParameterCheck {
    pub status: BackendObservationStatus,
    pub handshake: Option<BackendHandshakeObservation>,
    pub configured_pressure: Option<i32>,
    pub input_geometry: Option<BackendInputGeometryObservation>,
    pub frame_width: Option<u32>,
    pub frame_height: Option<u32>,
}

impl BackendInputParameterCheck {
    pub fn failed() -> Self {
        Self {
            status: BackendObservationStatus::Failed,
            ..Self::default()
        }
    }

    pub fn apply_to_report(&self, report: &mut BackendOpenReport) {
        report.input_check = self.status;
        report.handshake = self.handshake.clone();
        report.configured_pressure = self.configured_pressure;
        report.input_geometry = self.input_geometry.clone();
        if self.frame_width.is_some() {
            report.frame_width = self.frame_width;
            report.frame_height = self.frame_height;
        }
    }

    fn validate(&self, backend: &str) -> Result<(), SanitizationError> {
        let invalid = || SanitizationError::new("invalid_backend_input_check", "input_check");
        if !matches!(
            self.status,
            BackendObservationStatus::Passed | BackendObservationStatus::Failed
        ) || self.handshake.as_ref().is_some_and(|value| {
            value.max_contacts <= 0
                || value.max_x <= 0
                || value.max_y <= 0
                || value.max_pressure <= 0
        }) || self.input_geometry.as_ref().is_some_and(|value| {
            value.natural_max_x <= 0
                || value.natural_max_y <= 0
                || !matches!(value.rotation_degrees, 0 | 90 | 180 | 270)
        }) || self.frame_width.is_some() != self.frame_height.is_some()
            || self.frame_width == Some(0)
            || self.frame_height == Some(0)
        {
            return Err(invalid());
        }
        let matching_data = match backend {
            "maatouch" | "minitouch" => self.input_geometry.is_none() && self.frame_width.is_none(),
            "adb_shell_input" => {
                self.handshake.is_none()
                    && self.configured_pressure.is_none()
                    && self.frame_width.is_none()
            }
            "nemu_ipc" => {
                self.handshake.is_none()
                    && self.configured_pressure.is_none()
                    && self.input_geometry.is_none()
            }
            _ => false,
        };
        if !matching_data {
            return Err(invalid());
        }
        if self.status == BackendObservationStatus::Passed
            && !match backend {
                "maatouch" | "minitouch" => self.handshake.as_ref().is_some_and(|handshake| {
                    self.configured_pressure
                        .is_some_and(|pressure| pressure > 0 && pressure <= handshake.max_pressure)
                }),
                "adb_shell_input" => self.input_geometry.is_some(),
                "nemu_ipc" => self.frame_width.is_some() && self.frame_height.is_some(),
                _ => false,
            }
        {
            return Err(invalid());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendOpenSource {
    Native,
    Simulation,
    UnobservedProvider,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendInstallationSource {
    ExplicitFolder,
    ConfiguredBackendPath,
    RunningProcess,
    RegistryUninstall,
    VendorEnumeration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendOpenStage {
    Connect,
    Construct,
    CaptureProbe,
    CachedSelection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendOpenAttempt {
    pub backend: String,
    pub stage: BackendOpenStage,
    pub status: BackendObservationStatus,
    pub elapsed_ms: Option<u64>,
    pub detail: Option<LifecycleNativeDetail>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_parameters: Option<BackendInputParameterCheck>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendHandshakeObservation {
    pub max_contacts: i32,
    pub max_x: i32,
    pub max_y: i32,
    pub max_pressure: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendInputGeometryObservation {
    pub natural_max_x: i32,
    pub natural_max_y: i32,
    pub rotation_degrees: u16,
}

/// Original open and its same-command first capture. None/Unknown never grants availability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendOpenReport {
    /// Assigned by the kernel; meaningful with the event owner epoch and instance.
    pub session_generation: u64,
    pub entry: BackendOpenEntry,
    pub source: BackendOpenSource,
    pub requested: String,
    pub selected: Option<String>,
    pub status: BackendObservationStatus,
    pub connection: BackendObservationStatus,
    pub capture_check: BackendObservationStatus,
    #[serde(default, skip_serializing_if = "BackendObservationStatus::is_unknown")]
    pub input_check: BackendObservationStatus,
    pub handshake: Option<BackendHandshakeObservation>,
    pub input_geometry: Option<BackendInputGeometryObservation>,
    pub screen_size: Option<LifecycleNativeDetail>,
    pub frame_width: Option<u32>,
    pub frame_height: Option<u32>,
    pub serial_configured: Option<bool>,
    pub installation_source: Option<BackendInstallationSource>,
    pub configured_pressure: Option<i32>,
    pub attempts: Vec<BackendOpenAttempt>,
    pub dropped_attempts: u16,
    pub warnings: Vec<LifecycleNativeDetail>,
    pub dropped_warnings: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendOpenAttemptSummary {
    pub backend: String,
    pub stage: BackendOpenStage,
    pub status: BackendObservationStatus,
    pub elapsed_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_parameters: Option<BackendInputParameterCheck>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendOpenSummary {
    pub session_generation: u64,
    pub entry: BackendOpenEntry,
    pub source: BackendOpenSource,
    pub requested: String,
    pub selected: Option<String>,
    pub status: BackendObservationStatus,
    pub connection: BackendObservationStatus,
    pub capture_check: BackendObservationStatus,
    #[serde(default, skip_serializing_if = "BackendObservationStatus::is_unknown")]
    pub input_check: BackendObservationStatus,
    pub handshake: Option<BackendHandshakeObservation>,
    pub input_geometry: Option<BackendInputGeometryObservation>,
    pub frame_width: Option<u32>,
    pub frame_height: Option<u32>,
    pub serial_configured: Option<bool>,
    pub installation_source: Option<BackendInstallationSource>,
    pub configured_pressure: Option<i32>,
    pub attempts: Vec<BackendOpenAttemptSummary>,
    pub dropped_attempts: u16,
    pub warning_count: u32,
}

impl BackendOpenReport {
    pub(crate) fn public_summary(&self) -> BackendOpenSummary {
        BackendOpenSummary {
            session_generation: self.session_generation,
            entry: self.entry,
            source: self.source,
            requested: self.requested.clone(),
            selected: self.selected.clone(),
            status: self.status,
            connection: self.connection,
            capture_check: self.capture_check,
            input_check: self.input_check,
            handshake: self.handshake.clone(),
            input_geometry: self.input_geometry.clone(),
            frame_width: self.frame_width,
            frame_height: self.frame_height,
            serial_configured: self.serial_configured,
            installation_source: self.installation_source,
            configured_pressure: self.configured_pressure,
            attempts: self
                .attempts
                .iter()
                .map(|attempt| BackendOpenAttemptSummary {
                    backend: attempt.backend.clone(),
                    stage: attempt.stage,
                    status: attempt.status,
                    elapsed_ms: attempt.elapsed_ms,
                    input_parameters: attempt.input_parameters.clone(),
                })
                .collect(),
            dropped_attempts: self.dropped_attempts,
            warning_count: self.warnings.len() as u32 + u32::from(self.dropped_warnings),
        }
    }

    pub fn unobserved(entry: BackendOpenEntry) -> Self {
        Self {
            session_generation: 0,
            entry,
            source: BackendOpenSource::UnobservedProvider,
            requested: "unknown".into(),
            selected: None,
            status: BackendObservationStatus::Unknown,
            connection: BackendObservationStatus::Unknown,
            capture_check: BackendObservationStatus::Unknown,
            input_check: BackendObservationStatus::Unknown,
            handshake: None,
            input_geometry: None,
            screen_size: None,
            frame_width: None,
            frame_height: None,
            serial_configured: None,
            installation_source: None,
            configured_pressure: None,
            attempts: Vec::new(),
            dropped_attempts: 0,
            warnings: Vec::new(),
            dropped_warnings: 0,
        }
    }

    pub fn push_attempt(&mut self, attempt: BackendOpenAttempt) {
        if self.attempts.len() < 8 {
            self.attempts.push(attempt);
        } else {
            self.dropped_attempts = self.dropped_attempts.saturating_add(1);
        }
    }

    pub(crate) fn validate(&self) -> Result<(), SanitizationError> {
        let invalid = || SanitizationError::new("invalid_backend_open_observation", "backend_open");
        let token = |value: &str| {
            !value.is_empty()
                && value.len() <= 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        };
        if self.session_generation == 0
            || !token(&self.requested)
            || self.selected.as_deref().is_some_and(|value| !token(value))
            || self.attempts.len() > 8
            || (self.dropped_attempts > 0 && self.attempts.len() != 8)
            || self.warnings.len() > 8
            || (self.dropped_warnings > 0 && self.warnings.len() != 8)
            || self.frame_width.is_some() != self.frame_height.is_some()
            || self.frame_width == Some(0)
            || self.frame_height == Some(0)
            || (self.status == BackendObservationStatus::Passed && self.selected.is_none())
            || (self.entry == BackendOpenEntry::Input
                && self.capture_check == BackendObservationStatus::Passed)
            || (self.source != BackendOpenSource::Native
                && [self.status, self.connection, self.capture_check]
                    .contains(&BackendObservationStatus::Passed))
            || (self.source != BackendOpenSource::Simulation
                && [self.status, self.connection, self.capture_check]
                    .contains(&BackendObservationStatus::SimulationNotApplicable))
        {
            return Err(invalid());
        }
        if let Some(size) = &self.screen_size {
            size.validate()?;
        }
        if (self.entry == BackendOpenEntry::Capture
            && self.input_check != BackendObservationStatus::Unknown)
            || (self.source != BackendOpenSource::Native
                && matches!(
                    self.input_check,
                    BackendObservationStatus::Passed | BackendObservationStatus::Failed
                ))
            || (self.source != BackendOpenSource::Simulation
                && self.input_check == BackendObservationStatus::SimulationNotApplicable)
        {
            return Err(invalid());
        }
        if matches!(
            self.input_check,
            BackendObservationStatus::Passed | BackendObservationStatus::Failed
        ) {
            let backend = match self.entry {
                BackendOpenEntry::NemuPair
                    if self.requested == "nemu_ipc"
                        && (self.input_check != BackendObservationStatus::Passed
                            || self.selected.as_deref() == Some("nemu_ipc")) =>
                {
                    "nemu_ipc"
                }
                BackendOpenEntry::Input => self
                    .selected
                    .as_deref()
                    .or_else(|| self.attempts.last().map(|attempt| attempt.backend.as_str()))
                    .ok_or_else(invalid)?,
                _ => return Err(invalid()),
            };
            if self.entry == BackendOpenEntry::Input && backend == "nemu_ipc" {
                return Err(invalid());
            }
            BackendInputParameterCheck {
                status: self.input_check,
                handshake: self.handshake.clone(),
                configured_pressure: self.configured_pressure,
                input_geometry: self.input_geometry.clone(),
                frame_width: self.frame_width,
                frame_height: self.frame_height,
            }
            .validate(backend)?;
        }
        for warning in &self.warnings {
            warning.validate()?;
        }
        for attempt in &self.attempts {
            if let Some(check) = &attempt.input_parameters {
                if self.source != BackendOpenSource::Native
                    || self.entry == BackendOpenEntry::Capture
                    || attempt.stage != BackendOpenStage::Connect
                    || !matches!(
                        attempt.backend.as_str(),
                        "maatouch" | "minitouch" | "adb_shell_input"
                    )
                {
                    return Err(invalid());
                }
                check.validate(&attempt.backend)?;
            }
            if !token(&attempt.backend)
                || (self.source != BackendOpenSource::Native
                    && attempt.status == BackendObservationStatus::Passed)
                || (self.source != BackendOpenSource::Simulation
                    && attempt.status == BackendObservationStatus::SimulationNotApplicable)
                || (attempt.stage == BackendOpenStage::CachedSelection
                    && (attempt.status != BackendObservationStatus::Unknown
                        || attempt.elapsed_ms.is_some()))
            {
                return Err(invalid());
            }
            if let Some(detail) = &attempt.detail {
                detail.validate()?;
            }
        }
        Ok(())
    }
}
