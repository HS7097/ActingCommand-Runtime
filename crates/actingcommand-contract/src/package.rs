// SPDX-License-Identifier: AGPL-3.0-only

//! A package's identity is the `package_id` its `control.json` declares and a package is found
//! by its path (Workflow #336 option 丙). This module holds the path rules, the content hash
//! of the suspension rule and the bundle index of a standard package.

use crate::codes::RuntimeContractCode;
use crate::{RuntimeContractError, RuntimeContractResult};
use serde::{Deserialize, Serialize};

/// The domain line that opens every `content-directory.v1` content hash input.
pub const CONTENT_DIRECTORY_V1: &str = "actingcommand.package.content-directory.v1";

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

/// The single `content-directory.v1` digest: SHA-256 over the domain line followed by one
/// `<hex sha256 of the bytes>  <path>\n` line per regular file, ordered by the UTF-8 bytes
/// of the `/`-separated relative path. Callers supply distinct, already admitted paths.
/// Workflow #336 PKG-S5 (M-PKG v1.1 §3.3): the content hash of the suspension rule. Its one
/// caller is containment's `measure_content_hash`; no admission, tool or recording computes it.
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
    #[serde(rename = "actingcommand.bundle.v4")]
    V4,
}

/// The directory of a standard package's game root that holds its tables (Workflow #357
/// M-357a §2.1): a reserved name, so no pack of a bundle is named so.
pub const RESERVED_TABLES_DIRECTORY: &str = "tables";
/// The most table ids a bundle's `tables` list names (M-357a TB12).
pub const MAX_BUNDLE_TABLES: usize = 64;
/// The longest table id (M-357a §2.1: `[a-z][a-z0-9_]{0,63}`).
pub const MAX_BUNDLE_TABLE_ID_BYTES: usize = 64;

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
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidBundleSource,
            ));
        }
        Ok(())
    }
}

/// One task pack of a bundle (Workflow #336 PKG-S5): its content directory `path` is
/// `packs/<package_id>`. No digest names or identifies it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundlePack {
    pub package_id: String,
    pub server: String,
    pub entry_task_id: String,
    pub path: String,
    pub file_count: u64,
    pub byte_count: u64,
}

impl BundlePack {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        if !bundle_identifier(&self.server)
            || !bundle_text(&self.package_id)
            || !bundle_text(&self.entry_task_id)
            || self.file_count == 0
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidBundlePack,
            ));
        }
        if !package_id_names_one_path_segment(&self.package_id)
            || self
                .package_id
                .eq_ignore_ascii_case(RESERVED_TABLES_DIRECTORY)
                && false
            || self.path != format!("packs/{}", self.package_id)
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidBundlePackPath,
            ));
        }
        Ok(())
    }
}

/// Whether a `package_id` can name its package's directory or file (`packs/<package_id>`,
/// `<package_id>.<ext>`): one safe path segment, never `.` or `..`.
pub fn package_id_names_one_path_segment(package_id: &str) -> bool {
    package_id != "." && !package_id.contains('/') && safe_source_path(package_id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaintenanceUse {
    Startup,
    Prerequisite,
    ReturnHome,
    /// Workflow #384 D2-S2: the standard package's recognition maintenance package, the one
    /// package the scheduler runs to read an instance's facts (at most one per server).
    // Interface anchor `package` (contracts/component-interfaces.md): a reader without this use
    // refuses the declaration; A8 declares the revision once.
    Recognition,
}

crate::outcome_vocabulary! {
    /// Workflow #384 D2: why no reader can refresh an instance fact, or what is wrong with a
    /// reader binding (vocabulary `reader_problem`, `rd-1009/d2/model-d2-fact-refresh.md` §7.3).
    pub enum ReaderProblem: "reader_problem" {
        KeyNotMarked => "key_not_marked",
        DesignatedOperation => "designated_operation",
        NoMarks => "no_marks",
        RecognizeOnly => "recognize_only",
        StabilityTermination => "stability_termination",
        PolicyFactAuthority => "policy_fact_authority",
        LivePoolKey => "live_pool_key",
        PackageUnbound => "package_unbound",
        NotDeclared => "not_declared",
        Disabled => "disabled",
        ConfidenceBelowDocument => "confidence_below_document",
        IntervalExceedsFreshness => "interval_exceeds_freshness",
        /// Workflow #384 D2-S2: the bound package cannot be read or admitted.
        PackageUnreadable => "package_unreadable",
    }
}

/// Source declarations and the bundle index use this same shape. Machine and instance bindings
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
            || entry.uses.len() > 4
            || entry
                .uses
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != entry.uses.len()
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidBundleMaintenance,
            ));
        }
        for purpose in &entry.uses {
            if *purpose != MaintenanceUse::Prerequisite && !roles.insert((&entry.server, purpose)) {
                return Err(RuntimeContractError::new(
                    RuntimeContractCode::DuplicateBundleMaintenanceRole,
                ));
            }
        }
    }
    Ok(())
}

/// `bundle.json` of a standard package's resource section (Workflow #288; bundle v4, Workflow #336
/// PKG-S5): each pack named by its `package_id`, in the directory `packs/<package_id>`. A file
/// format only, never persisted in the ledger. It names no default package: the applications
/// table beside it does (`servers.<server>.default_package_id`). Earlier versions (v1, v2 and
/// v3) are other shapes no reader of this build accepts.
// Interface anchor `package` (contracts/component-interfaces.md): raise its revision in
// distribution/windows/component-interfaces.json when packs need what older readers lack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleIndex {
    pub schema_version: BundleIndexVersion,
    pub game: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<BundleSource>,
    pub packs: Vec<BundlePack>,
    /// The bundle's maintenance declarations (`package bundle --maintenance`), each naming a
    /// pack by its `package_id`; absent when the bundle declares none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maintenance: Option<Vec<BundleMaintenance>>,
    /// Workflow #357 M-357a TB12: the tables the standard package ships in the game root's
    /// `tables/` directory, by table id. Only an updater uses it (it replaces exactly these
    /// tables and leaves the others of the root alone); the Runtime parses and checks it and
    /// otherwise ignores it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tables: Option<Vec<String>>,
}

impl BundleIndex {
    /// At least one pack; package ids unique; every `path` is `packs/<package_id>` and no pack
    /// is named `tables`; maintenance declarations close over the packs; the table list holds
    /// at most 64 unique table ids.
    pub fn validate(&self) -> RuntimeContractResult<()> {
        if !bundle_identifier(&self.game) || self.packs.is_empty() {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidBundleIndex,
            ));
        }
        if let Some(source) = &self.source {
            source.validate()?;
        }
        let mut package_ids = std::collections::BTreeSet::new();
        for pack in &self.packs {
            pack.validate()?;
            if !package_ids.insert(pack.package_id.as_str()) {
                return Err(RuntimeContractError::new(
                    RuntimeContractCode::DuplicateBundlePack,
                ));
            }
        }
        let maintenance = self.maintenance();
        if maintenance.len() > self.packs.len() {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidBundleMaintenance,
            ));
        }
        validate_bundle_maintenance_declarations(maintenance)?;
        for entry in maintenance {
            if !self
                .packs
                .iter()
                .any(|pack| pack.package_id == entry.package_id && pack.server == entry.server)
            {
                return Err(RuntimeContractError::new(
                    RuntimeContractCode::BundleMaintenancePackMissing,
                ));
            }
        }
        if let Some(tables) = &self.tables {
            let unique = tables
                .iter()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>();
            if tables.len() > MAX_BUNDLE_TABLES
                || unique.len() != tables.len()
                || !tables.iter().all(|table_id| bundle_table_id(table_id))
            {
                return Err(RuntimeContractError::new(
                    RuntimeContractCode::InvalidBundleIndex,
                ));
            }
        }
        Ok(())
    }

    /// The maintenance declarations, none when the bundle declares none.
    pub fn maintenance(&self) -> &[BundleMaintenance] {
        self.maintenance.as_deref().unwrap_or_default()
    }
}

/// A table id of a bundle's `tables` list (M-357a §2.1): 1-64 bytes of `[a-z][a-z0-9_]*`.
fn bundle_table_id(value: &str) -> bool {
    let mut bytes = value.bytes();
    value.len() <= MAX_BUNDLE_TABLE_ID_BYTES
        && bytes.next().is_some_and(|first| first.is_ascii_lowercase())
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

/// A game or server key of a bundle: 1-128 bytes of `[a-z0-9._-]`, usable as one path segment.
pub(crate) fn bundle_identifier(value: &str) -> bool {
    value.len() <= 128
        && !value.contains('/')
        && safe_source_path(value)
        && value != "."
        && value
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"._-".contains(&c))
}

/// A package or task id of a bundle: 1-128 bytes, trimmed, without control characters.
pub(crate) fn bundle_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

// Workflow #336 PKG-S5 (exec key 66): bundle v4, tests S5-T2 and S5-T6.
#[cfg(test)]
#[path = "package_tests.rs"]
mod tests;
