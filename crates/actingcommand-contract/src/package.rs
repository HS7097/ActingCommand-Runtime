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
        let repository = self
            .repository
            .strip_prefix("https://")
            .ok_or_else(|| RuntimeContractError::new("invalid_package_repository"))?;
        let (host, path) = repository
            .split_once('/')
            .ok_or_else(|| RuntimeContractError::new("invalid_package_repository"))?;
        if self.repository.len() > 512
            || host.is_empty()
            || host != host.to_ascii_lowercase()
            || !host
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b".-".contains(&c))
            || !safe_source_path(path)
            || !path
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'/' | b'.'))
            || path.ends_with(".git")
            || path == "."
            || self.commit.algorithm != self.tree.algorithm
            || !safe_source_path(&self.bundle_path)
        {
            return Err(RuntimeContractError::new("invalid_source_tree_reference"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ContentDirectoryVersion {
    #[serde(rename = "actingcommand.package.content-directory.v1")]
    V1,
}

/// The domain line that opens every `content-directory.v1` digest input.
pub const CONTENT_DIRECTORY_V1: &str = "actingcommand.package.content-directory.v1";

/// A package identified only by the content of its directory: no repository, commit or
/// path participates. `sha256` is the lowercase hex `content_directory_digest`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentDirectory {
    pub schema_version: ContentDirectoryVersion,
    pub sha256: String,
}

impl ContentDirectory {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.sha256.len() != 64 || !lower_hex(&self.sha256) {
            return Err(RuntimeContractError::new(
                "invalid_content_directory_reference",
            ));
        }
        Ok(())
    }
}

/// The legacy string encoding preserves existing record bytes. Versioned references
/// carry their own wire version and the entire identity in the same typed slot. The
/// untagged variants are tried in order, so new variants are only ever appended.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PackageRef {
    LegacyZipSha256(String),
    GitSourceTree(Box<GitSourceTree>),
    ContentDirectory(ContentDirectory),
}

// Older policy payloads can omit the binding. Keep them decodable; every
// admission/recovery validation rejects this unresolved value.
impl Default for PackageRef {
    fn default() -> Self {
        Self::LegacyZipSha256(String::new())
    }
}

impl PackageRef {
    pub fn prefixed_wire_value(&self) -> serde_json::Value {
        match self {
            Self::LegacyZipSha256(hash) if hash.is_empty() => {
                serde_json::Value::String(String::new())
            }
            Self::LegacyZipSha256(hash) => serde_json::Value::String(format!("sha256:{hash}")),
            Self::GitSourceTree(reference) => serde_json::json!(reference),
            Self::ContentDirectory(reference) => serde_json::json!(reference),
        }
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        match self {
            Self::LegacyZipSha256(value) => {
                if value.len() != 64 || !lower_hex(value) {
                    return Err(RuntimeContractError::new("invalid_package_sha256"));
                }
                Ok(())
            }
            Self::GitSourceTree(value) => value.validate(),
            Self::ContentDirectory(value) => value.validate(),
        }
    }

    pub fn legacy_sha256(&self) -> Option<&str> {
        match self {
            Self::LegacyZipSha256(value) => Some(value),
            Self::GitSourceTree(_) | Self::ContentDirectory(_) => None,
        }
    }

    /// The locator is handed to containment, which reads it by path (a directory or a content
    /// container file, Workflow #336), rather than read here as a ZIP file.
    pub fn is_directory_source(&self) -> bool {
        matches!(self, Self::GitSourceTree(_) | Self::ContentDirectory(_))
    }

    pub fn parse_argument(value: &str) -> RuntimeContractResult<Self> {
        let reference = if value.trim_start().starts_with('{') {
            // A JSON object can only decode as one of the versioned object references.
            serde_json::from_str(value)
                .map_err(|_| RuntimeContractError::new("invalid_source_tree_reference"))?
        } else {
            Self::LegacyZipSha256(value.strip_prefix("sha256:").unwrap_or(value).to_owned())
        };
        reference.validate()?;
        Ok(reference)
    }
}

impl From<String> for PackageRef {
    fn from(value: String) -> Self {
        Self::LegacyZipSha256(value.strip_prefix("sha256:").unwrap_or(&value).to_owned())
    }
}

impl From<&str> for PackageRef {
    fn from(value: &str) -> Self {
        value.to_owned().into()
    }
}

impl From<&String> for PackageRef {
    fn from(value: &String) -> Self {
        value.clone().into()
    }
}

impl From<&PackageRef> for PackageRef {
    fn from(value: &PackageRef) -> Self {
        value.clone()
    }
}

pub fn safe_source_path(value: &str) -> bool {
    value == "."
        || (!value.is_empty()
            && value.len() <= 1024
            && value.split('/').count() <= 64
            && value.split('/').all(|part| {
                !part.is_empty()
                    && part != "."
                    && part != ".."
                    && !part.ends_with(['.', ' '])
                    && !part.starts_with('-')
                    && part
                        .chars()
                        .all(|c| !c.is_control() && !"<>:\"\\|?*".contains(c))
                    && !matches!(
                        part.split('.')
                            .next()
                            .unwrap_or("")
                            .to_ascii_uppercase()
                            .as_str(),
                        "CON"
                            | "PRN"
                            | "AUX"
                            | "NUL"
                            | "COM1"
                            | "COM2"
                            | "COM3"
                            | "COM4"
                            | "COM5"
                            | "COM6"
                            | "COM7"
                            | "COM8"
                            | "COM9"
                            | "LPT1"
                            | "LPT2"
                            | "LPT3"
                            | "LPT4"
                            | "LPT5"
                            | "LPT6"
                            | "LPT7"
                            | "LPT8"
                            | "LPT9"
                    )
            }))
}

fn lower_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// The final path segment when it has the digest form (64 lowercase hex digits).
pub fn digest_named(path: &std::path::Path) -> Option<&str> {
    path.file_name()?
        .to_str()
        .filter(|name| name.len() == 64 && lower_hex(name))
}

/// Workflow #336: the final path segment without its content container extension (`.zip` or
/// `.json`, ASCII case-insensitive) when what remains has the digest form.
pub fn digest_named_stem(path: &std::path::Path) -> Option<&str> {
    let (stem, extension) = path.file_name()?.to_str()?.rsplit_once('.')?;
    (["zip", "json"]
        .iter()
        .any(|container| extension.eq_ignore_ascii_case(container))
        && stem.len() == 64
        && lower_hex(stem))
    .then_some(stem)
}

/// The single `content-directory.v1` digest: SHA-256 over the domain line followed by one
/// `<hex sha256 of the bytes>  <path>\n` line per regular file, ordered by the UTF-8 bytes
/// of the `/`-separated relative path. Callers supply distinct, already admitted paths.
pub fn content_directory_digest<'a>(
    files: impl IntoIterator<Item = (&'a str, [u8; 32])>,
) -> String {
    use sha2::{Digest, Sha256};
    let mut files: Vec<_> = files.into_iter().collect();
    // `str` ordering is the byte order of its UTF-8 encoding (`LC_ALL=C sort`).
    files.sort_unstable_by_key(|file| file.0);
    let mut hash = Sha256::new();
    hash.update(CONTENT_DIRECTORY_V1.as_bytes());
    hash.update(b"\n");
    for (path, file) in files {
        hash.update(lower_hex_string(&file).as_bytes());
        hash.update(b"  ");
        hash.update(path.as_bytes());
        hash.update(b"\n");
    }
    lower_hex_string(&hash.finalize())
}

fn lower_hex_string(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BundleIndexVersion {
    #[serde(rename = "actingcommand.bundle.v2")]
    V2,
}

/// Where a bundle's packs were built from. Information only: it never takes part in a
/// pack's identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleSource {
    /// `<owner>/<name>`.
    pub repository: String,
    /// The lowercase hex commit id.
    pub commit: String,
}

impl BundleSource {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        let segment = |value: &str| {
            !value.is_empty()
                && value.len() <= 100
                && value != "."
                && value != ".."
                && value
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
        };
        let repository = self
            .repository
            .split_once('/')
            .is_some_and(|(owner, name)| segment(owner) && segment(name));
        if !repository || !matches!(self.commit.len(), 40 | 64) || !lower_hex(&self.commit) {
            return Err(RuntimeContractError::new("invalid_bundle_source"));
        }
        Ok(())
    }
}

/// One task pack of a bundle: the content directory `path` (`packs/<digest>`), whose
/// `content-directory.v1` digest is `digest`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundlePackV2 {
    pub package_id: String,
    pub server: String,
    pub entry_task_id: String,
    pub digest: String,
    pub path: String,
    pub file_count: u64,
    pub byte_count: u64,
}

impl BundlePackV2 {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        if !bundle_identifier(&self.server)
            || !bundle_text(&self.package_id)
            || !bundle_text(&self.entry_task_id)
            || self.file_count == 0
        {
            return Err(RuntimeContractError::new("invalid_bundle_pack"));
        }
        if self.digest.len() != 64
            || !lower_hex(&self.digest)
            || self.path != format!("packs/{}", self.digest)
        {
            return Err(RuntimeContractError::new("invalid_bundle_pack_path"));
        }
        Ok(())
    }
}

/// `bundle.json` of a standard package's resource section (Workflow #288): each package id
/// mapped to its hash-named content directory. A file format only, never persisted in the
/// ledger. It names no default package: the applications table beside it does
/// (`servers.<server>.default_package_id`). Version 1 remains a separate shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleIndexV2 {
    pub schema_version: BundleIndexVersion,
    pub game: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<BundleSource>,
    pub packs: Vec<BundlePackV2>,
}

impl BundleIndexV2 {
    /// At least one pack; package ids and digests unique; every `path` is `packs/<digest>`.
    pub fn validate(&self) -> RuntimeContractResult<()> {
        if !bundle_identifier(&self.game) || self.packs.is_empty() {
            return Err(RuntimeContractError::new("invalid_bundle_index"));
        }
        if let Some(source) = &self.source {
            source.validate()?;
        }
        let mut package_ids = std::collections::BTreeSet::new();
        let mut digests = std::collections::BTreeSet::new();
        for pack in &self.packs {
            pack.validate()?;
            if !package_ids.insert(pack.package_id.as_str())
                || !digests.insert(pack.digest.as_str())
            {
                return Err(RuntimeContractError::new("duplicate_bundle_pack"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaintenanceUse {
    Startup,
    Prerequisite,
    ReturnHome,
}

/// Source declarations and bundle v3 use this same shape. Machine and instance bindings
/// belong to the consuming installer, not to a resource bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleMaintenance {
    pub package_id: String,
    pub server: String,
    pub uses: Vec<MaintenanceUse>,
}

/// Rules decidable from a source maintenance array alone. Bundle reference closure and
/// actual package qualification are checked by the index and execution-kernel owners.
pub fn validate_bundle_maintenance_declarations(
    maintenance: &[BundleMaintenance],
) -> RuntimeContractResult<()> {
    let mut entries = std::collections::BTreeSet::new();
    let mut roles = std::collections::BTreeSet::new();
    for entry in maintenance {
        if !bundle_text(&entry.package_id)
            || !bundle_identifier(&entry.server)
            || !entries.insert((&entry.package_id, &entry.server))
            || entry.uses.is_empty()
            || entry.uses.len() > 3
            || entry
                .uses
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != entry.uses.len()
        {
            return Err(RuntimeContractError::new("invalid_bundle_maintenance"));
        }
        for purpose in &entry.uses {
            if *purpose != MaintenanceUse::Prerequisite && !roles.insert((&entry.server, purpose)) {
                return Err(RuntimeContractError::new(
                    "duplicate_bundle_maintenance_role",
                ));
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BundleIndexV3Version {
    #[serde(rename = "actingcommand.bundle.v3")]
    V3,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleIndexV3 {
    pub schema_version: BundleIndexV3Version,
    pub game: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<BundleSource>,
    pub packs: Vec<BundlePackV2>,
    pub maintenance: Vec<BundleMaintenance>,
}

impl BundleIndexV3 {
    /// Structural closure only; actual material admission and use eligibility are owned by
    /// execution-kernel and must also pass before a consumer installs these bindings.
    pub fn validate(&self) -> RuntimeContractResult<()> {
        BundleIndexV2 {
            schema_version: BundleIndexVersion::V2,
            game: self.game.clone(),
            source: self.source.clone(),
            packs: self.packs.clone(),
        }
        .validate()?;
        if self.maintenance.len() > self.packs.len() {
            return Err(RuntimeContractError::new("invalid_bundle_maintenance"));
        }
        validate_bundle_maintenance_declarations(&self.maintenance)?;
        for entry in &self.maintenance {
            if !self
                .packs
                .iter()
                .any(|pack| pack.package_id == entry.package_id && pack.server == entry.server)
            {
                return Err(RuntimeContractError::new("bundle_maintenance_pack_missing"));
            }
        }
        Ok(())
    }
}

// Interface anchor `package` (contracts/component-interfaces.md): raise its revision in
// distribution/windows/component-interfaces.json when packs need what older readers lack.
/// Version-specific shapes keep required v3 fields mandatory and keep v2 strict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BundleIndex {
    V3(BundleIndexV3),
    V2(BundleIndexV2),
}

impl BundleIndex {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        match self {
            Self::V2(index) => index.validate(),
            Self::V3(index) => index.validate(),
        }
    }
}

/// A game or server key of a bundle: 1-128 bytes of `[a-z0-9._-]`, usable as one path segment.
fn bundle_identifier(value: &str) -> bool {
    value.len() <= 128
        && !value.contains('/')
        && safe_source_path(value)
        && value != "."
        && value
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"._-".contains(&c))
}

/// A package or task id of a bundle: 1-128 bytes, trimmed, without control characters.
fn bundle_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

/// Existing prefixed ZIP digest slots retain their canonical bytes while
/// source-tree values carry the common versioned reference.
pub mod prefixed_reference {
    use super::*;
    use serde::{Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &PackageRef, serializer: S) -> Result<S::Ok, S::Error> {
        value.prefixed_wire_value().serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<PackageRef, D::Error> {
        let value = PackageRef::deserialize(deserializer)?;
        match value {
            PackageRef::LegacyZipSha256(hash) if hash.is_empty() => {
                Ok(PackageRef::LegacyZipSha256(hash))
            }
            PackageRef::LegacyZipSha256(hash) => {
                let hash = hash.strip_prefix("sha256:").ok_or_else(|| {
                    serde::de::Error::custom("policy package digest requires sha256 prefix")
                })?;
                Ok(hash.into())
            }
            source => Ok(source),
        }
    }
}

pub mod optional_prefixed_reference {
    use super::*;
    use serde::{Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        value: &Option<PackageRef>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value
            .as_ref()
            .map(PackageRef::prefixed_wire_value)
            .serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<PackageRef>, D::Error> {
        let value = Option::<PackageRef>::deserialize(deserializer)?;
        match value {
            Some(PackageRef::LegacyZipSha256(hash)) if hash.is_empty() => {
                Ok(Some(PackageRef::LegacyZipSha256(hash)))
            }
            Some(PackageRef::LegacyZipSha256(hash)) => {
                let hash = hash.strip_prefix("sha256:").ok_or_else(|| {
                    serde::de::Error::custom("policy package digest requires sha256 prefix")
                })?;
                Ok(Some(hash.into()))
            }
            source => Ok(source),
        }
    }
}
