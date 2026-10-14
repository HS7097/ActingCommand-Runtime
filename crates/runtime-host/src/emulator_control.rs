// SPDX-License-Identifier: AGPL-3.0-only

use crate::codes::{HostCode, HostLocation};
use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::RuntimeErrorCode;
use actingcommand_device::{
    EmulatorCapability, EmulatorCapabilityBackend, EmulatorCapabilityProfile,
};
use std::collections::BTreeSet;

/// Admits a read-only emulator capability profile without granting provider process authority.
pub fn admit_emulator_capabilities(
    backend: &mut dyn EmulatorCapabilityBackend,
    required_capability_ids: &[&str],
) -> RuntimeHostResult<EmulatorCapabilityProfile> {
    if required_capability_ids.is_empty()
        || required_capability_ids.len() > EmulatorCapability::ALL.len()
    {
        return Err(RuntimeHostError::fatal(
            HostCode::InvalidEmulatorCapabilityRequirementCount,
            HostLocation::AdmitEmulatorCapabilities,
            RuntimeErrorCode::RuntimeHalted,
        ));
    }

    let mut required = BTreeSet::new();
    for capability_id in required_capability_ids {
        let capability = capability_id.parse::<EmulatorCapability>().map_err(|_| {
            RuntimeHostError::fatal(
                HostCode::UnknownEmulatorCapability,
                HostLocation::AdmitEmulatorCapabilities,
                RuntimeErrorCode::RuntimeHalted,
            )
        })?;
        if !required.insert(capability) {
            return Err(RuntimeHostError::fatal(
                HostCode::DuplicateEmulatorCapabilityRequirement,
                HostLocation::AdmitEmulatorCapabilities,
                RuntimeErrorCode::RuntimeHalted,
            ));
        }
    }

    let profile = backend.probe_capabilities().map_err(|_| {
        RuntimeHostError::fatal(
            HostCode::EmulatorCapabilityProbeFailed,
            HostLocation::AdmitEmulatorCapabilities,
            RuntimeErrorCode::RuntimeHalted,
        )
    })?;
    for capability in required {
        profile.capability(capability).map_err(|_| {
            RuntimeHostError::fatal(
                HostCode::EmulatorCapabilityUnavailable,
                HostLocation::AdmitEmulatorCapabilities,
                RuntimeErrorCode::RuntimeHalted,
            )
        })?;
    }
    Ok(profile)
}

#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_device::{
        DeviceError, EmulatorCapabilityAvailability, EmulatorCapabilityEvidence,
        EmulatorVersionEvidence, EmulatorVersionUnavailableReason, FakeEmulatorCapabilityBackend,
    };

    #[test]
    fn host_admits_available_fake_capabilities_without_device_authority() {
        let mut backend = fake_backend(EmulatorCapabilityAvailability::Available);

        let profile =
            admit_emulator_capabilities(&mut backend, &["inventory.read", "instance.status.read"])
                .expect("available fake capabilities");

        assert_eq!(profile.provider_id(), "neutral.fake");
        assert_eq!(backend.probe_count(), 1);
    }

    #[test]
    fn host_rejects_unknown_capability_before_backend_probe() {
        let mut backend = fake_backend(EmulatorCapabilityAvailability::Available);

        let error = admit_emulator_capabilities(&mut backend, &["instance.teleport"])
            .expect_err("unknown capability must fail");

        assert!(error.is_fatal());
        assert_eq!(error.code(), "unknown_emulator_capability");
        assert_eq!(backend.probe_count(), 0);
    }

    #[test]
    fn host_rejects_unverified_capability() {
        let mut backend = fake_backend(EmulatorCapabilityAvailability::Unverified);

        let error = admit_emulator_capabilities(&mut backend, &["instance.start"])
            .expect_err("unverified capability must fail");

        assert!(error.is_fatal());
        assert_eq!(error.code(), "emulator_capability_unavailable");
        assert_eq!(backend.probe_count(), 1);
    }

    #[test]
    fn host_propagates_fake_provider_probe_failure() {
        let mut backend = fake_backend(EmulatorCapabilityAvailability::Available);
        backend.fail_next_probe(DeviceError::fatal("synthetic provider failure"));

        let error = admit_emulator_capabilities(&mut backend, &["inventory.read"])
            .expect_err("provider failure must propagate");

        assert!(error.is_fatal());
        assert_eq!(error.code(), "emulator_capability_probe_failed");
        assert_eq!(backend.probe_count(), 1);
    }

    fn fake_backend(availability: EmulatorCapabilityAvailability) -> FakeEmulatorCapabilityBackend {
        let evidence = EmulatorCapability::ALL
            .into_iter()
            .map(|capability| {
                EmulatorCapabilityEvidence::new(
                    capability,
                    availability,
                    "offline fake response is explicit",
                    "https://example.invalid/emulator-contract",
                )
                .expect("capability evidence")
            })
            .collect();
        let profile = EmulatorCapabilityProfile::new(
            "neutral.fake",
            EmulatorVersionEvidence::Unavailable {
                reason: EmulatorVersionUnavailableReason::NotProbed,
            },
            evidence,
        )
        .expect("fake profile");
        FakeEmulatorCapabilityBackend::new(profile)
    }
}
