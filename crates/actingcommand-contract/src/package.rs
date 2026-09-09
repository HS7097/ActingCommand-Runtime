// SPDX-License-Identifier: AGPL-3.0-only

//! Immutable package identity, independent of the local material locator.

use crate::{RuntimeContractError, RuntimeContractResult};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GitObjectAlgorithm {
    Sha1,
    Sha256,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitOid {
    pub algorithm: GitObjectAlgorithm,
    pub hex: String,
}

impl GitOid {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        let length = match self.algorithm {
            GitObjectAlgorithm::Sha1 => 40,
            GitObjectAlgorithm::Sha256 => 64,
        };
        if self.hex.len() != length || !lower_hex(&self.hex) {
            return Err(RuntimeContractError::new("invalid_git_object_id"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum GitSourceTreeVersion {
    #[serde(rename = "actingcommand.package.git-source-tree.v1")]
    V1,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitSourceTree {
    pub schema_version: GitSourceTreeVersion,
    pub repository: String,
    pub commit: GitOid,
    pub bundle_path: String,
    pub tree: GitOid,
}

impl GitSourceTree {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        self.commit.validate()?;
        self.tree.validate()?;
        let repository = self.repository.strip_prefix("https://")
            .ok_or_else(|| RuntimeContractError::new("invalid_package_repository"))?;
        let (host, path) = repository.split_once('/')
            .ok_or_else(|| RuntimeContractError::new("invalid_package_repository"))?;
        if self.repository.len() > 512 || host.is_empty()
            || host != host.to_ascii_lowercase()
            || !host.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b".-".contains(&c))
            || !safe_source_path(path) || path.ends_with(".git")
            || path == "." || self.commit.algorithm != self.tree.algorithm
            || !safe_source_path(&self.bundle_path)
        {
            return Err(RuntimeContractError::new("invalid_source_tree_reference"));
        }
        Ok(())
    }
}

/// The legacy string encoding preserves existing record bytes. Source references
/// carry their own wire version and the entire Git identity in the same typed slot.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PackageRef {
    LegacyZipSha256(String),
    GitSourceTree(GitSourceTree),
}

// Older policy payloads can omit the binding. Keep them decodable; every
// admission/recovery validation rejects this unresolved value.
impl Default for PackageRef {
    fn default() -> Self { Self::LegacyZipSha256(String::new()) }
}

impl PackageRef {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        match self {
            Self::LegacyZipSha256(value) => {
                if value.len() != 64 || !lower_hex(value) {
                    return Err(RuntimeContractError::new("invalid_package_sha256"));
                }
                Ok(())
            }
            Self::GitSourceTree(value) => value.validate(),
        }
    }

    pub fn legacy_sha256(&self) -> Option<&str> {
        match self {
            Self::LegacyZipSha256(value) => Some(value),
            Self::GitSourceTree(_) => None,
        }
    }

    pub fn parse_argument(value: &str) -> RuntimeContractResult<Self> {
        let reference = if value.starts_with('{') {
            serde_json::from_str(value)
                .map(Self::GitSourceTree)
                .map_err(|_| RuntimeContractError::new("invalid_source_tree_reference"))?
        } else {
            Self::LegacyZipSha256(value.strip_prefix("sha256:").unwrap_or(value).to_owned())
        };
        reference.validate()?;
        Ok(reference)
    }
}

impl From<String> for PackageRef {
    fn from(value: String) -> Self { Self::LegacyZipSha256(value.strip_prefix("sha256:").unwrap_or(&value).to_owned()) }
}

impl From<&str> for PackageRef {
    fn from(value: &str) -> Self { value.to_owned().into() }
}

impl From<&String> for PackageRef {
    fn from(value: &String) -> Self { value.clone().into() }
}

impl From<&PackageRef> for PackageRef {
    fn from(value: &PackageRef) -> Self { value.clone() }
}

pub fn safe_source_path(value: &str) -> bool {
    value == "." || (!value.is_empty() && value.len() <= 1024
        && value.split('/').count() <= 64
        && value.split('/').all(|part| {
            !part.is_empty() && part != "." && part != ".." && !part.ends_with(['.', ' '])
                && !part.starts_with('-')
                && part.bytes().all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
                && !matches!(part.split('.').next().unwrap_or("").to_ascii_uppercase().as_str(),
                    "CON" | "PRN" | "AUX" | "NUL" | "COM1" | "COM2" | "COM3" | "COM4" |
                    "COM5" | "COM6" | "COM7" | "COM8" | "COM9" | "LPT1" | "LPT2" | "LPT3" |
                    "LPT4" | "LPT5" | "LPT6" | "LPT7" | "LPT8" | "LPT9")
        }))
}

fn lower_hex(value: &str) -> bool {
    value.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// Policy's existing wire uses a prefixed ZIP digest. Its canonical bytes are
/// retained while source-tree values carry the common versioned reference.
pub mod policy_reference {
    use super::*;
    use serde::{Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &PackageRef, serializer: S) -> Result<S::Ok, S::Error> {
        match value {
            PackageRef::LegacyZipSha256(hash) => serializer.serialize_str(&format!("sha256:{hash}")),
            PackageRef::GitSourceTree(_) => value.serialize(serializer),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<PackageRef, D::Error> {
        let value = PackageRef::deserialize(deserializer)?;
        match value {
            PackageRef::LegacyZipSha256(hash) => {
                let hash = hash.strip_prefix("sha256:").ok_or_else(|| serde::de::Error::custom("policy package digest requires sha256 prefix"))?;
                Ok(hash.into())
            }
            source => Ok(source),
        }
    }
}

pub mod optional_policy_reference {
    use super::*;
    use serde::{Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &Option<PackageRef>, serializer: S) -> Result<S::Ok, S::Error> {
        match value {
            Some(PackageRef::LegacyZipSha256(hash)) => serializer.serialize_some(&format!("sha256:{hash}")),
            _ => value.serialize(serializer),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<PackageRef>, D::Error> {
        let value = Option::<PackageRef>::deserialize(deserializer)?;
        match value {
            Some(PackageRef::LegacyZipSha256(hash)) => {
                let hash = hash.strip_prefix("sha256:").ok_or_else(|| serde::de::Error::custom("policy package digest requires sha256 prefix"))?;
                Ok(Some(hash.into()))
            }
            source => Ok(source),
        }
    }
}
