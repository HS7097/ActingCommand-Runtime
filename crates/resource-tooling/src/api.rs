// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{BundleIndex, BundleMaintenance, BundleSource};
use serde::Serialize;
use serde_json::Value;
use std::path::PathBuf;

/// The 32 MiB bound for package maintenance input and resource-restoration draft payloads.
pub const DEFAULT_MAX_BUFFERED_PAYLOAD_BYTES: usize = 32 * 1024 * 1024;

/// Workflow #288 A2b `package bundle`: the applications table, the directory holding one
/// source directory per task pack, and the new output directory.
#[derive(Debug, Clone)]
pub struct PackageBundleRequest {
    pub applications: PathBuf,
    pub packs_root: PathBuf,
    pub out: PathBuf,
    pub source: Option<BundleSource>,
    /// Present (including an empty list) writes the bundle's `maintenance` declarations and
    /// requests actual maintenance qualification; absent writes none (bundle v4 either way,
    /// Workflow #336 PKG-S5).
    pub maintenance: Option<Vec<BundleMaintenance>>,
    /// Workflow #373 NAV-T: the standard-pack root files to lay beside `bundle.json`.
    pub root_files: StandardPackRootFiles,
}

/// Workflow #373 NAV-T (M-373 v1.1 §3.1, §3.9): the root files `package bundle` checks and lays
/// beside `bundle.json` byte for byte, each by its source path (`--pages`, `--interrupts`,
/// `--navigation`); none given lays none.
#[derive(Debug, Clone, Default)]
pub struct StandardPackRootFiles {
    pub pages: Option<PathBuf>,
    pub interrupts: Option<PathBuf>,
    pub navigation: Option<PathBuf>,
}

/// Workflow #373 NAV-T: `actinglab standard-pack check --root <game root>`, an installed game
/// root `<standard_packs.root>/<game>/`.
#[derive(Debug, Clone)]
pub struct StandardPackCheckRequest {
    pub root: PathBuf,
}

/// Workflow #373 NAV-T: a game root that passed the checks `package bundle` runs.
#[derive(Debug, Clone, Serialize)]
pub struct StandardPackCheckResponse {
    pub status: String,
    pub root: String,
    pub game: String,
    /// The root files the game root holds.
    pub root_files: Vec<&'static str>,
    pub pack_count: usize,
    /// The report "entry page not in the list": one `package_bundle_entry_page_unlisted` per
    /// package whose entry pages `pages.json` does not all list (none without `pages.json`);
    /// then (Workflow #411 B15) one `package_bundle_remedy_invalid` per remedy finding.
    pub warnings: Vec<actingcommand_contract::outcome::Outcome>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PackageBundleResponse {
    pub status: String,
    pub out: String,
    pub index: BundleIndex,
    /// Workflow #384 D2-S2: one `recognition_pack_undeclared` per pack that reads like a
    /// recognition maintenance package of a server that declares none (Workflow #411 A32: only
    /// the one a server declares counts); Workflow #373 NAV-T: with
    /// `--pages`, one `package_bundle_entry_page_unlisted` per pack whose entry pages the page
    /// list does not all list; Workflow #411 B15: with any root file, one
    /// `package_bundle_remedy_invalid` per remedy finding. Absent when there is none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<actingcommand_contract::outcome::Outcome>,
}

#[derive(Debug, Clone)]
/// Workflow #336 PKG-S4 (option 丙): the package by its path; no hash.
pub struct PackageValidateRequest {
    pub zip_path: PathBuf,
    pub include_entries: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PackageValidationResponse {
    pub status: String,
    pub module: String,
    pub manifest_path: String,
    pub task_count: usize,
    pub entry_count: usize,
    pub dangerous_entries: Vec<String>,
    pub recognition_pack_diagnostics: Vec<RecognitionPackDiagnosticsResponse>,
    pub manifest: JsonDocument,
    pub entries: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecognitionPackDiagnosticsResponse {
    pub path: String,
    pub unsupported_target_count: usize,
    pub unsupported_targets: Vec<UnsupportedRecognitionTargetResponse>,
}

/// A template target with recognition semantics this build cannot evaluate (Workflow #378
/// A2-P: typed members replace the English `reason`).
#[derive(Debug, Clone, Serialize)]
pub struct UnsupportedRecognitionTargetResponse {
    pub id: String,
    /// The declared matching method in the pack spelling (`rgb_count`, `hsv_count`), when it
    /// is not `ncc`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<actingcommand_recognition_pack::RecognitionMethod>,
    /// Whether the target declares a mask.
    pub mask: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(transparent)]
pub struct JsonDocument(Value);

impl JsonDocument {
    pub(crate) fn new(value: Value) -> Self {
        Self(value)
    }
}
