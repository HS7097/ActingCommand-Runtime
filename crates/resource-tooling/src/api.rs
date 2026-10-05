// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{BundleIndex, BundleMaintenance, BundleSource, ContentDirectory};
use actingcommand_pack_containment::Sha256Hash;
use serde::Serialize;
use serde_json::Value;
use std::path::PathBuf;

/// The 32 MiB bound for package maintenance input and resource-restoration draft payloads.
pub const DEFAULT_MAX_BUFFERED_PAYLOAD_BYTES: usize = 32 * 1024 * 1024;

/// Workflow #288 A2b `package digest`: one local package directory.
#[derive(Debug, Clone)]
pub struct PackageDigestRequest {
    pub package: PathBuf,
}

/// The directory's content-directory reference, from a full admission against it.
#[derive(Debug, Clone, Serialize)]
pub struct PackageDigestResponse {
    pub status: String,
    pub package: String,
    pub reference: ContentDirectory,
    pub package_id: String,
    pub server: String,
    pub entry_task_id: String,
    pub file_count: usize,
    pub byte_count: u64,
}

/// Workflow #288 A2b `package bundle`: the applications table, the directory holding one
/// source directory per task pack, and the new output directory.
#[derive(Debug, Clone)]
pub struct PackageBundleRequest {
    pub applications: PathBuf,
    pub packs_root: PathBuf,
    pub out: PathBuf,
    pub source: Option<BundleSource>,
    /// Present (including an empty list) requests v3 and actual maintenance qualification.
    /// Absent retains the v2 generation path.
    pub maintenance: Option<Vec<BundleMaintenance>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PackageBundleResponse {
    pub status: String,
    pub out: String,
    pub index: BundleIndex,
}

#[derive(Debug, Clone)]
pub struct PackageValidateRequest {
    pub zip_path: PathBuf,
    pub include_entries: bool,
    pub expected_input_sha256: Option<Sha256Hash>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PackageValidationResponse {
    pub status: String,
    pub input_sha256: String,
    pub hash_source: String,
    pub externally_verified: bool,
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

#[derive(Debug, Clone, Serialize)]
pub struct UnsupportedRecognitionTargetResponse {
    pub id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(transparent)]
pub struct JsonDocument(Value);

impl JsonDocument {
    pub(crate) fn new(value: Value) -> Self {
        Self(value)
    }
}
