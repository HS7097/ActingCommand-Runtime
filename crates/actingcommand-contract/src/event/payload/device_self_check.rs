// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::{DeviceSelfCheckEntry, DeviceSelfCheckFailure, DeviceSelfCheckStatus};

/// The frame size a capture (or Nemu pair) open verified with its first frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceSelfCheckCapture {
    pub width: u32,
    pub height: u32,
}

/// The touch bounds an input (or Nemu pair) open read back from its connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceSelfCheckTouch {
    pub max_x: i32,
    pub max_y: i32,
}

/// What one recorded backend open says about the device connection (Workflow #317 sc3): the
/// host derives it from the open's report at the site that records the
/// `backend.selfcheck.<entry>.*` facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceSelfCheck {
    pub instance_alias: String,
    pub entry: DeviceSelfCheckEntry,
    pub status: DeviceSelfCheckStatus,
    pub selected: Option<String>,
    pub capture: Option<DeviceSelfCheckCapture>,
    pub touch: Option<DeviceSelfCheckTouch>,
    pub failure_code: Option<DeviceSelfCheckFailure>,
    pub generation: u64,
}

/// `device.self_check` (Workflow #317 sc3): a status hint, one per recorded backend open of an
/// entry, carrying the same status as that open's `backend.selfcheck.<entry>.status` fact. It
/// holds no frame. `failure_code` is present exactly when `status` is `failed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceSelfCheckPayload {
    instance_alias: String,
    entry: DeviceSelfCheckEntry,
    status: DeviceSelfCheckStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    selected: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    capture: Option<DeviceSelfCheckCapture>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    touch: Option<DeviceSelfCheckTouch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    failure_code: Option<DeviceSelfCheckFailure>,
    generation: u64,
    audit: SanitizedAudit,
}

impl DeviceSelfCheckPayload {
    pub fn instance_alias(&self) -> &str {
        &self.instance_alias
    }

    pub const fn entry(&self) -> DeviceSelfCheckEntry {
        self.entry
    }

    pub const fn status(&self) -> DeviceSelfCheckStatus {
        self.status
    }

    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    pub const fn capture(&self) -> Option<DeviceSelfCheckCapture> {
        self.capture
    }

    pub const fn touch(&self) -> Option<DeviceSelfCheckTouch> {
        self.touch
    }

    pub const fn failure_code(&self) -> Option<DeviceSelfCheckFailure> {
        self.failure_code
    }

    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub(super) fn validate(&self) -> Result<(), SanitizationError> {
        let invalid = || SanitizationError::new("invalid_device_self_check", "device_self_check");
        crate::validate_instance_alias(&self.instance_alias).map_err(|_| invalid())?;
        let backend_name = |value: &str| {
            !value.is_empty()
                && value.len() <= 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        };
        let captures = matches!(
            self.entry,
            DeviceSelfCheckEntry::Capture | DeviceSelfCheckEntry::Nemu
        );
        let touches = matches!(
            self.entry,
            DeviceSelfCheckEntry::Input | DeviceSelfCheckEntry::Nemu
        );
        let failure_matches_entry = matches!(
            (self.entry, self.failure_code),
            (_, None)
                | (
                    DeviceSelfCheckEntry::Input,
                    Some(
                        DeviceSelfCheckFailure::InputBackendOpenFailed
                            | DeviceSelfCheckFailure::InputCheckFailed,
                    ),
                )
                | (
                    DeviceSelfCheckEntry::Capture,
                    Some(
                        DeviceSelfCheckFailure::CaptureBackendOpenFailed
                            | DeviceSelfCheckFailure::CaptureCheckFailed,
                    ),
                )
                | (
                    DeviceSelfCheckEntry::Nemu,
                    Some(
                        DeviceSelfCheckFailure::PairedBackendOpenFailed
                            | DeviceSelfCheckFailure::InputCheckFailed
                            | DeviceSelfCheckFailure::CaptureCheckFailed,
                    ),
                )
        );
        if self
            .selected
            .as_deref()
            .is_some_and(|value| !backend_name(value))
            || self
                .capture
                .is_some_and(|capture| !captures || capture.width == 0 || capture.height == 0)
            || self
                .touch
                .is_some_and(|touch| !touches || touch.max_x <= 0 || touch.max_y <= 0)
            || self.failure_code.is_some() != (self.status == DeviceSelfCheckStatus::Failed)
            || !failure_matches_entry
        {
            return Err(invalid());
        }
        Ok(())
    }
}

impl PayloadDetail for DeviceSelfCheckPayload {
    fn action(&self) -> EventAction {
        EventAction::DeviceSelfCheck
    }
    fn diagnostic_code(&self) -> Option<DiagnosticCode> {
        None
    }
    fn effect_disposition(&self) -> Option<EffectDisposition> {
        None
    }
    fn audit(&self) -> &SanitizedAudit {
        &self.audit
    }
}

pub(super) struct DeviceSelfCheckDraft {
    check: DeviceSelfCheck,
    audit: AuditInput,
}

impl RuntimePayloadDraft {
    /// One `device.self_check` status hint (Workflow #317 sc3).
    pub fn device_self_check(check: DeviceSelfCheck) -> Self {
        Self(RuntimeDraftKind::DeviceSelfCheck(DeviceSelfCheckDraft {
            check,
            audit: AuditInput::new(),
        }))
    }
}

impl DeviceSelfCheckDraft {
    pub(super) fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<DeviceSelfCheckPayload, SanitizationError> {
        let DeviceSelfCheck {
            instance_alias,
            entry,
            status,
            selected,
            capture,
            touch,
            failure_code,
            generation,
        } = self.check;
        let payload = DeviceSelfCheckPayload {
            instance_alias,
            entry,
            status,
            selected,
            capture,
            touch,
            failure_code,
            generation,
            audit: self.audit.sanitize(fingerprinter)?,
        };
        payload.validate()?;
        Ok(payload)
    }
}
