// SPDX-License-Identifier: AGPL-3.0-only

//! #402 HS-2 (design HS-M §3.5; register §1.270): `device.backend_switched`, the record of one
//! instance whose touch and capture backends a `ConfigureSettings` switched. A new closed action
//! and payload under the lifecycle event type, and with them the closed vocabulary
//! `nemu_app_index_source`: `ledger` revision 6 (#402 HS-2b, register §1.274;
//! contracts/component-interfaces.md). A reader of [3, 5] refuses such a record (the action and
//! the payload deny unknown values), so the record is a new revision, not an additive change.

use super::*;
use crate::codes::SanitizationCode;

/// One backend of the instance before and after the switch, by the device layer's own names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceBackendChange {
    pub from: String,
    pub to: String,
}

/// Where the Nemu application index the instance opens Nemu touch with came from: the
/// configuration file's entry, or (HS-3) resolved by the Runtime when it opened the paired
/// session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NemuAppIndexSource {
    File,
    Resolved,
}

/// What one switch did (#402 HS-2): the instance, its two backends before and after, the Nemu
/// application index when the touch backend after it is Nemu IPC and the instance has one, and
/// the generation of the session the switch's connection preparation opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceBackendSwitched {
    pub instance_alias: String,
    pub touch: DeviceBackendChange,
    pub capture: DeviceBackendChange,
    pub nemu_app_index: Option<u32>,
    pub nemu_app_index_source: Option<NemuAppIndexSource>,
    pub session_generation: u64,
}

/// `device.backend_switched` (#402 HS-2): recorded after `host.config_overrides` and
/// `config.parameters.instance`, before the command's `command.validated`, only when the switch
/// committed. Its sensitivity is `internal`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceBackendSwitchedPayload {
    instance_alias: String,
    touch: DeviceBackendChange,
    capture: DeviceBackendChange,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    nemu_app_index: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    nemu_app_index_source: Option<NemuAppIndexSource>,
    session_generation: u64,
    audit: SanitizedAudit,
}

impl DeviceBackendSwitchedPayload {
    pub fn instance_alias(&self) -> &str {
        &self.instance_alias
    }

    pub const fn touch(&self) -> &DeviceBackendChange {
        &self.touch
    }

    pub const fn capture(&self) -> &DeviceBackendChange {
        &self.capture
    }

    pub const fn nemu_app_index(&self) -> Option<u32> {
        self.nemu_app_index
    }

    pub const fn nemu_app_index_source(&self) -> Option<NemuAppIndexSource> {
        self.nemu_app_index_source
    }

    pub const fn session_generation(&self) -> u64 {
        self.session_generation
    }

    /// A valid alias, backend names that are tokens, at least one backend changed, and an index
    /// exactly with its source.
    pub(super) fn validate(&self) -> Result<(), SanitizationError> {
        let invalid = || {
            SanitizationError::new(
                SanitizationCode::InvalidSanitizedPayload,
                "device_backend_switched",
            )
        };
        crate::validate_instance_alias(&self.instance_alias).map_err(|_| invalid())?;
        let backend_name = |value: &str| {
            !value.is_empty()
                && value.len() <= 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        };
        let names = [
            &self.touch.from,
            &self.touch.to,
            &self.capture.from,
            &self.capture.to,
        ];
        if names.iter().any(|name| !backend_name(name))
            || (self.touch.from == self.touch.to && self.capture.from == self.capture.to)
            || self.nemu_app_index.is_some() != self.nemu_app_index_source.is_some()
        {
            return Err(invalid());
        }
        Ok(())
    }
}

impl PayloadDetail for DeviceBackendSwitchedPayload {
    fn action(&self) -> EventAction {
        EventAction::DeviceBackendSwitched
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

pub(super) struct DeviceBackendSwitchedDraft {
    switched: DeviceBackendSwitched,
    audit: AuditInput,
}

impl RuntimePayloadDraft {
    /// One `device.backend_switched` record (#402 HS-2).
    pub fn device_backend_switched(switched: DeviceBackendSwitched) -> Self {
        Self(RuntimeDraftKind::DeviceBackendSwitched(
            DeviceBackendSwitchedDraft {
                switched,
                audit: AuditInput::new(),
            },
        ))
    }
}

impl DeviceBackendSwitchedDraft {
    pub(super) fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<DeviceBackendSwitchedPayload, SanitizationError> {
        let DeviceBackendSwitched {
            instance_alias,
            touch,
            capture,
            nemu_app_index,
            nemu_app_index_source,
            session_generation,
        } = self.switched;
        let payload = DeviceBackendSwitchedPayload {
            instance_alias,
            touch,
            capture,
            nemu_app_index,
            nemu_app_index_source,
            session_generation,
            audit: self.audit.sanitize(fingerprinter)?,
        };
        payload.validate()?;
        Ok(payload)
    }
}
