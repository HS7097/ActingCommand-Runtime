// SPDX-License-Identifier: AGPL-3.0-only

//! Production package ingress over the pack-containment capability.

use actingcommand_pack_containment::{
    Containment, ContainmentError, InstanceId, LoadedBundle, Sha256Hash,
};
use actingcommand_recognition_pack::VisionProvider;
use std::error::Error;
use std::fmt;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExternalExpectedSha256(Sha256Hash);

impl ExternalExpectedSha256 {
    pub fn parse_hex(value: &str) -> Result<Self, ContainmentError> {
        Sha256Hash::parse_hex(value).map(Self)
    }

    pub const fn hash(self) -> Sha256Hash {
        self.0
    }
}

#[derive(Debug)]
pub enum ExecutionBundleError {
    Containment(ContainmentError),
    MissingLoadedBundle,
}

impl fmt::Display for ExecutionBundleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Containment(error) => error.fmt(formatter),
            Self::MissingLoadedBundle => formatter.write_str(
                "fatal execution bundle error: containment did not retain the loaded bundle",
            ),
        }
    }
}

impl Error for ExecutionBundleError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Containment(error) => Some(error),
            Self::MissingLoadedBundle => None,
        }
    }
}

impl From<ContainmentError> for ExecutionBundleError {
    fn from(error: ContainmentError) -> Self {
        Self::Containment(error)
    }
}

/// Immutable production resource capability admitted with an externally supplied expected hash.
#[derive(Debug)]
pub struct ExternallyVerifiedBundle {
    bundle: LoadedBundle,
}

impl ExternallyVerifiedBundle {
    pub fn load(
        instance_label: &str,
        zip_bytes: &[u8],
        expected: ExternalExpectedSha256,
    ) -> Result<Self, ExecutionBundleError> {
        Self::load_with_optional_vision_provider(instance_label, zip_bytes, expected, None)
    }

    pub fn load_with_vision_provider(
        instance_label: &str,
        zip_bytes: &[u8],
        expected: ExternalExpectedSha256,
        vision_provider: Arc<dyn VisionProvider>,
    ) -> Result<Self, ExecutionBundleError> {
        Self::load_with_optional_vision_provider(
            instance_label,
            zip_bytes,
            expected,
            Some(vision_provider),
        )
    }

    fn load_with_optional_vision_provider(
        instance_label: &str,
        zip_bytes: &[u8],
        expected: ExternalExpectedSha256,
        vision_provider: Option<Arc<dyn VisionProvider>>,
    ) -> Result<Self, ExecutionBundleError> {
        let instance = InstanceId::new(instance_label)?;
        let mut containment = match vision_provider {
            Some(provider) => Containment::with_vision_provider(provider),
            None => Containment::new(),
        };
        containment.load(&instance, zip_bytes, &expected.hash())?;
        let bundle = containment
            .take_loaded(&instance)
            .ok_or(ExecutionBundleError::MissingLoadedBundle)?;
        Ok(Self { bundle })
    }

    pub fn loaded_bundle(&self) -> &LoadedBundle {
        &self.bundle
    }

    pub fn into_loaded_bundle(self) -> LoadedBundle {
        self.bundle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_hash_type_rejects_invalid_hash_text() {
        assert!(ExternalExpectedSha256::parse_hex("not-a-hash").is_err());
    }

    #[test]
    fn production_ingress_rejects_hash_mismatch_before_zip_parsing() {
        let expected = ExternalExpectedSha256::parse_hex(
            "0000000000000000000000000000000000000000000000000000000000000000",
        )
        .expect("expected hash");
        let error = ExternallyVerifiedBundle::load("node.a", b"not-a-zip", expected)
            .expect_err("hash mismatch");

        assert!(matches!(
            error,
            ExecutionBundleError::Containment(ContainmentError::HashMismatch { .. })
        ));
    }
}
