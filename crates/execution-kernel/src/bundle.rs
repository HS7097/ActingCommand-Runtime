// SPDX-License-Identifier: AGPL-3.0-only

//! Production package ingress over the pack-containment capability.

use actingcommand_pack_containment::{
    Containment, ContainmentError, InstanceId, LoadedBundle, Sha256Hash,
};
use std::error::Error;
use std::fmt;

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
        let instance = InstanceId::new(instance_label)?;
        let mut containment = Containment::new();
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
        let error = ExternallyVerifiedBundle::load("ak.cn", b"not-a-zip", expected)
            .expect_err("hash mismatch");

        assert!(matches!(
            error,
            ExecutionBundleError::Containment(ContainmentError::HashMismatch { .. })
        ));
    }
}
