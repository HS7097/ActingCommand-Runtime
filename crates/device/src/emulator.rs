// SPDX-License-Identifier: AGPL-3.0-only

//! Offline capability provider boundary. The shared evidence types have no device authority.

use crate::{DeviceError, DeviceResult};
pub use actingcommand_contract::emulator::*;

impl From<EmulatorCapabilityError> for DeviceError {
    fn from(error: EmulatorCapabilityError) -> Self {
        Self::fatal(error.message())
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

        let error = DeviceError::from(error);
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

        let error = DeviceError::from(error);
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

        let error = DeviceError::from(error);
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

        let error = DeviceError::from(error);
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
