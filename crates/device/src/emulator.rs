// SPDX-License-Identifier: AGPL-3.0-only

//! Offline capability contracts for future emulator control-plane adapters.
//!
//! This module has no process, network, ADB, or device authority. Real provider
//! adapters remain deferred until their command and failure contracts are frozen.

use crate::{DeviceError, DeviceResult};
use serde::Serialize;
use std::collections::BTreeMap;
use std::str::FromStr;

pub const EMULATOR_CAPABILITY_SCHEMA_VERSION: &str = "actingcommand.emulator-capabilities.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EmulatorCapability {
    InventoryRead,
    InstanceStatusRead,
    InstanceStart,
    InstanceStop,
    InstanceRestart,
    InstanceCreate,
    InstanceClone,
    InstanceDelete,
    InstanceConfigure,
    ApplicationControl,
    AdbBridge,
    SnapshotManage,
}

impl EmulatorCapability {
    pub const ALL: [Self; 12] = [
        Self::InventoryRead,
        Self::InstanceStatusRead,
        Self::InstanceStart,
        Self::InstanceStop,
        Self::InstanceRestart,
        Self::InstanceCreate,
        Self::InstanceClone,
        Self::InstanceDelete,
        Self::InstanceConfigure,
        Self::ApplicationControl,
        Self::AdbBridge,
        Self::SnapshotManage,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InventoryRead => "inventory.read",
            Self::InstanceStatusRead => "instance.status.read",
            Self::InstanceStart => "instance.start",
            Self::InstanceStop => "instance.stop",
            Self::InstanceRestart => "instance.restart",
            Self::InstanceCreate => "instance.create",
            Self::InstanceClone => "instance.clone",
            Self::InstanceDelete => "instance.delete",
            Self::InstanceConfigure => "instance.configure",
            Self::ApplicationControl => "application.control",
            Self::AdbBridge => "adb.bridge",
            Self::SnapshotManage => "snapshot.manage",
        }
    }
}

impl FromStr for EmulatorCapability {
    type Err = DeviceError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "inventory.read" => Ok(Self::InventoryRead),
            "instance.status.read" => Ok(Self::InstanceStatusRead),
            "instance.start" => Ok(Self::InstanceStart),
            "instance.stop" => Ok(Self::InstanceStop),
            "instance.restart" => Ok(Self::InstanceRestart),
            "instance.create" => Ok(Self::InstanceCreate),
            "instance.clone" => Ok(Self::InstanceClone),
            "instance.delete" => Ok(Self::InstanceDelete),
            "instance.configure" => Ok(Self::InstanceConfigure),
            "application.control" => Ok(Self::ApplicationControl),
            "adb.bridge" => Ok(Self::AdbBridge),
            "snapshot.manage" => Ok(Self::SnapshotManage),
            other => Err(DeviceError::fatal(format!(
                "unknown emulator capability {other:?} for {EMULATOR_CAPABILITY_SCHEMA_VERSION}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EmulatorVersionEvidence {
    Exact { value: String },
    Minimum { value: String },
    Unavailable { reason: String },
}

impl EmulatorVersionEvidence {
    fn validate(&self) -> DeviceResult<()> {
        match self {
            Self::Exact { value } | Self::Minimum { value } => {
                validate_bounded_text(value, "provider version", 64)
            }
            Self::Unavailable { reason } => {
                validate_bounded_text(reason, "unavailable version reason", 256)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EmulatorCapabilityAvailability {
    Available,
    Unavailable,
    Unverified,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EmulatorCapabilityEvidence {
    capability: EmulatorCapability,
    availability: EmulatorCapabilityAvailability,
    failure_semantics: String,
    evidence_ref: String,
}

impl EmulatorCapabilityEvidence {
    pub fn new(
        capability: EmulatorCapability,
        availability: EmulatorCapabilityAvailability,
        failure_semantics: impl Into<String>,
        evidence_ref: impl Into<String>,
    ) -> DeviceResult<Self> {
        let evidence = Self {
            capability,
            availability,
            failure_semantics: failure_semantics.into(),
            evidence_ref: evidence_ref.into(),
        };
        validate_bounded_text(&evidence.failure_semantics, "failure semantics", 512)?;
        validate_bounded_text(&evidence.evidence_ref, "evidence reference", 512)?;
        Ok(evidence)
    }

    pub const fn capability(&self) -> EmulatorCapability {
        self.capability
    }

    pub const fn availability(&self) -> EmulatorCapabilityAvailability {
        self.availability
    }

    pub fn failure_semantics(&self) -> &str {
        &self.failure_semantics
    }

    pub fn evidence_ref(&self) -> &str {
        &self.evidence_ref
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EmulatorCapabilityProfile {
    schema_version: String,
    provider_id: String,
    version: EmulatorVersionEvidence,
    capabilities: BTreeMap<EmulatorCapability, EmulatorCapabilityEvidence>,
}

impl EmulatorCapabilityProfile {
    pub fn new(
        provider_id: impl Into<String>,
        version: EmulatorVersionEvidence,
        evidence: Vec<EmulatorCapabilityEvidence>,
    ) -> DeviceResult<Self> {
        let provider_id = provider_id.into();
        validate_provider_id(&provider_id)?;
        version.validate()?;

        let mut capabilities = BTreeMap::new();
        for entry in evidence {
            let capability = entry.capability();
            if capabilities.insert(capability, entry).is_some() {
                return Err(DeviceError::fatal(format!(
                    "duplicate emulator capability evidence for {}",
                    capability.as_str()
                )));
            }
        }
        let missing = EmulatorCapability::ALL
            .into_iter()
            .filter(|capability| !capabilities.contains_key(capability))
            .map(EmulatorCapability::as_str)
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(DeviceError::fatal(format!(
                "emulator capability profile is incomplete; missing {}",
                missing.join(", ")
            )));
        }

        Ok(Self {
            schema_version: EMULATOR_CAPABILITY_SCHEMA_VERSION.to_string(),
            provider_id,
            version,
            capabilities,
        })
    }

    pub fn schema_version(&self) -> &str {
        &self.schema_version
    }

    pub fn provider_id(&self) -> &str {
        &self.provider_id
    }

    pub const fn version(&self) -> &EmulatorVersionEvidence {
        &self.version
    }

    pub fn capability(
        &self,
        capability: EmulatorCapability,
    ) -> DeviceResult<&EmulatorCapabilityEvidence> {
        let evidence = self.capabilities.get(&capability).ok_or_else(|| {
            DeviceError::fatal(format!(
                "emulator capability profile is missing {}",
                capability.as_str()
            ))
        })?;
        match evidence.availability() {
            EmulatorCapabilityAvailability::Available => Ok(evidence),
            availability => Err(DeviceError::fatal(format!(
                "emulator provider {} cannot claim {}: {availability:?}; {}",
                self.provider_id,
                capability.as_str(),
                evidence.failure_semantics()
            ))),
        }
    }

    pub fn capability_by_id(
        &self,
        capability_id: &str,
    ) -> DeviceResult<&EmulatorCapabilityEvidence> {
        self.capability(capability_id.parse()?)
    }

    pub fn evidence(&self, capability: EmulatorCapability) -> &EmulatorCapabilityEvidence {
        self.capabilities
            .get(&capability)
            .expect("validated profiles contain every closed capability")
    }
}

/// Read-only provider boundary used before a real emulator adapter receives process authority.
pub trait EmulatorCapabilityBackend {
    fn probe_capabilities(&mut self) -> DeviceResult<EmulatorCapabilityProfile>;
}

/// Deterministic offline backend for Runtime contract rehearsals and tests.
#[derive(Debug, Clone)]
pub struct FakeEmulatorCapabilityBackend {
    profile: EmulatorCapabilityProfile,
    next_failure: Option<DeviceError>,
    probe_count: u64,
}

impl FakeEmulatorCapabilityBackend {
    pub const fn new(profile: EmulatorCapabilityProfile) -> Self {
        Self {
            profile,
            next_failure: None,
            probe_count: 0,
        }
    }

    pub fn fail_next_probe(&mut self, error: DeviceError) {
        self.next_failure = Some(error);
    }

    pub const fn probe_count(&self) -> u64 {
        self.probe_count
    }
}

impl EmulatorCapabilityBackend for FakeEmulatorCapabilityBackend {
    fn probe_capabilities(&mut self) -> DeviceResult<EmulatorCapabilityProfile> {
        self.probe_count = self
            .probe_count
            .checked_add(1)
            .ok_or_else(|| DeviceError::fatal("emulator capability probe counter overflowed"))?;
        if let Some(error) = self.next_failure.take() {
            return Err(error);
        }
        Ok(self.profile.clone())
    }
}

fn validate_provider_id(value: &str) -> DeviceResult<()> {
    validate_bounded_text(value, "provider id", 64)?;
    if value
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte))
    {
        Ok(())
    } else {
        Err(DeviceError::fatal(format!(
            "invalid emulator provider id {value:?}; expected lowercase ASCII token"
        )))
    }
}

fn validate_bounded_text(value: &str, field: &str, max_len: usize) -> DeviceResult<()> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(DeviceError::fatal(format!("{field} must not be empty")));
    }
    if trimmed.len() > max_len {
        return Err(DeviceError::fatal(format!(
            "{field} exceeds {max_len} bytes"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DeviceErrorSeverity;

    const SOURCE: &str = "https://example.invalid/emulator-contract";

    #[test]
    fn complete_profile_exposes_available_capability() {
        let profile = neutral_profile(EmulatorCapabilityAvailability::Available);

        let evidence = profile
            .capability_by_id("instance.status.read")
            .expect("available capability");

        assert_eq!(
            evidence.capability(),
            EmulatorCapability::InstanceStatusRead
        );
        assert_eq!(profile.schema_version(), EMULATOR_CAPABILITY_SCHEMA_VERSION);
        assert_eq!(profile.provider_id(), "neutral.fake");
    }

    #[test]
    fn unavailable_capability_fails_loud() {
        let profile = neutral_profile(EmulatorCapabilityAvailability::Unavailable);

        let error = profile
            .capability(EmulatorCapability::InstanceStart)
            .expect_err("unavailable capability must fail");

        assert_eq!(error.severity(), DeviceErrorSeverity::Fatal);
        assert!(error.message().contains("instance.start"));
        assert!(error.message().contains("Unavailable"));
    }

    #[test]
    fn unknown_capability_id_fails_loud() {
        let profile = neutral_profile(EmulatorCapabilityAvailability::Available);

        let error = profile
            .capability_by_id("instance.teleport")
            .expect_err("unknown capability must fail");

        assert_eq!(error.severity(), DeviceErrorSeverity::Fatal);
        assert!(error.message().contains("unknown emulator capability"));
    }

    #[test]
    fn incomplete_profile_is_rejected() {
        let error = EmulatorCapabilityProfile::new(
            "neutral.fake",
            unavailable_version(),
            vec![evidence(
                EmulatorCapability::InventoryRead,
                EmulatorCapabilityAvailability::Available,
            )],
        )
        .expect_err("incomplete capability matrix must fail");

        assert_eq!(error.severity(), DeviceErrorSeverity::Fatal);
        assert!(error.message().contains("profile is incomplete"));
    }

    #[test]
    fn duplicate_capability_is_rejected() {
        let duplicate = evidence(
            EmulatorCapability::InventoryRead,
            EmulatorCapabilityAvailability::Available,
        );
        let error = EmulatorCapabilityProfile::new(
            "neutral.fake",
            unavailable_version(),
            vec![duplicate.clone(), duplicate],
        )
        .expect_err("duplicate capability evidence must fail");

        assert_eq!(error.severity(), DeviceErrorSeverity::Fatal);
        assert!(error.message().contains("duplicate emulator capability"));
    }

    #[test]
    fn fake_backend_rehearses_without_external_authority() {
        let profile = neutral_profile(EmulatorCapabilityAvailability::Available);
        let mut backend = FakeEmulatorCapabilityBackend::new(profile.clone());

        let observed = backend
            .probe_capabilities()
            .expect("offline fake probe should succeed");

        assert_eq!(observed, profile);
        assert_eq!(backend.probe_count(), 1);
    }

    #[test]
    fn fake_backend_propagates_probe_failure() {
        let profile = neutral_profile(EmulatorCapabilityAvailability::Available);
        let mut backend = FakeEmulatorCapabilityBackend::new(profile);
        backend.fail_next_probe(DeviceError::fatal("synthetic provider failure"));

        let error = backend
            .probe_capabilities()
            .expect_err("configured failure must propagate");

        assert_eq!(error.severity(), DeviceErrorSeverity::Fatal);
        assert_eq!(error.message(), "synthetic provider failure");
        assert_eq!(backend.probe_count(), 1);
    }

    #[test]
    fn capability_ids_round_trip_without_aliases() {
        for capability in EmulatorCapability::ALL {
            let parsed = capability
                .as_str()
                .parse::<EmulatorCapability>()
                .expect("known capability id");
            assert_eq!(parsed, capability);
        }
    }

    fn neutral_profile(availability: EmulatorCapabilityAvailability) -> EmulatorCapabilityProfile {
        EmulatorCapabilityProfile::new(
            "neutral.fake",
            unavailable_version(),
            EmulatorCapability::ALL
                .into_iter()
                .map(|capability| evidence(capability, availability))
                .collect(),
        )
        .expect("neutral profile")
    }

    fn unavailable_version() -> EmulatorVersionEvidence {
        EmulatorVersionEvidence::Unavailable {
            reason: "offline fake has no vendor version".to_string(),
        }
    }

    fn evidence(
        capability: EmulatorCapability,
        availability: EmulatorCapabilityAvailability,
    ) -> EmulatorCapabilityEvidence {
        EmulatorCapabilityEvidence::new(
            capability,
            availability,
            "offline fake response is explicit",
            SOURCE,
        )
        .expect("capability evidence")
    }
}
