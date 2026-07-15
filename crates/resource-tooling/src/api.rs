// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_pack_containment::Sha256Hash;
use serde::Serialize;
use serde_json::Value;
use std::path::PathBuf;

/// A source entry or all generated package payloads may use at most 32 MiB.
/// The supported minimum packaging environment reserves 512 MiB of process
/// headroom; this one-sixteenth share leaves room for conversion, ZIP state,
/// and validation while source assets use the fixed streaming buffer.
pub const DEFAULT_MAX_BUFFERED_PAYLOAD_BYTES: usize = 32 * 1024 * 1024;

/// Maximum source-payload chunk handed to the ZIP compressor at one time.
pub const PACKAGE_COMPRESSOR_INPUT_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub enum PackageSource {
    Local(PathBuf),
    Remote(String),
}

#[derive(Debug, Clone, Default)]
pub struct PackageEnvOptions {
    pub instance: Option<String>,
    pub game: Option<String>,
    pub server: Option<String>,
    pub env_task: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct PackageResolution {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone)]
pub struct PackageBuildTaskRequest {
    pub source: PackageSource,
    pub temporary_root: PathBuf,
    pub task_id: String,
    pub game: Option<String>,
    pub server: Option<String>,
    pub locale: Option<String>,
    pub package_id: Option<String>,
    pub execution_mode: Option<String>,
    pub resolution: Option<PackageResolution>,
    pub include_recovery: bool,
    pub out: PathBuf,
    pub dry_run: bool,
    pub max_buffered_payload_bytes: usize,
    pub env: PackageEnvOptions,
}

#[derive(Debug, Clone, Serialize)]
pub struct PackageBuildTaskResponse {
    pub status: String,
    pub mode: String,
    pub repo: String,
    pub resource_root: String,
    pub resource_layout: String,
    pub from_remote: Option<String>,
    pub task_id: String,
    pub included_tasks: Vec<String>,
    pub game: String,
    pub server: String,
    pub package_id: String,
    pub execution_mode: String,
    pub dry_run: bool,
    pub out: Option<String>,
    pub validation: LabPackageValidationResponse,
}

#[derive(Debug, Clone)]
pub struct PackageBuildCatalogRequest {
    pub source: PackageSource,
    pub temporary_root: PathBuf,
    pub game: Option<String>,
    pub server: Option<String>,
    pub locale: Option<String>,
    pub max_buffered_payload_bytes: usize,
}

#[derive(Debug, Clone)]
pub struct PackageTaskArchiveRequest {
    pub task_id: String,
    pub package_id: String,
    pub execution_mode: String,
    pub resolution: Option<PackageResolution>,
    pub out: PathBuf,
    pub dry_run: bool,
    pub env: PackageEnvOptions,
}

#[derive(Debug, Clone)]
pub struct PackageFullArchiveRequest {
    pub entry_task_id: String,
    pub package_id: String,
    pub execution_mode: String,
    pub resolution: Option<PackageResolution>,
    pub out: PathBuf,
    pub dry_run: bool,
    pub env: PackageEnvOptions,
}

#[derive(Debug, Clone)]
pub struct PackageBuildCatalogMetadata {
    pub repo: PathBuf,
    pub resource_root: PathBuf,
    pub resource_layout: String,
    pub from_remote: Option<String>,
    pub game: String,
    pub server: String,
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
pub struct LabPackageValidationResponse {
    pub zip: String,
    pub status: String,
    pub entry_count: usize,
    pub control: LabPackageControlResponse,
    pub resources: LabPackageResourcesResponse,
}

#[derive(Debug, Clone, Serialize)]
pub struct LabPackageControlResponse {
    pub package_id: String,
    pub execution_mode: String,
    pub game: String,
    pub server: String,
    pub resolution: PackageResolution,
    pub entry_task_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LabPackageResourcesResponse {
    pub resource_root: String,
    pub manifest: String,
    pub operation: String,
    pub operation_count: usize,
    pub pack: String,
    pub recognition_unsupported_target_count: usize,
    pub recognition_unsupported_targets: Vec<UnsupportedRecognitionTargetResponse>,
    pub pages: String,
    pub navigation: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ResourceConvertRequest {
    pub repo: PathBuf,
    pub game: Option<String>,
    pub server: Option<String>,
    pub locale: Option<String>,
    pub maa_tasks_root: Option<PathBuf>,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResourceConvertResponse {
    pub repo: String,
    pub resource_root: String,
    pub resource_layout: String,
    pub game: String,
    pub server: String,
    pub locale: String,
    pub dry_run: bool,
    pub bundles: usize,
    pub targets: usize,
    pub pages: usize,
    pub edges: usize,
    pub page_operations: usize,
    pub index_tasks: usize,
    pub primitives: usize,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maa_tasks_root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maa_compiled_tasks: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(transparent)]
pub struct JsonDocument(Value);

impl JsonDocument {
    pub(crate) fn new(value: Value) -> Self {
        Self(value)
    }
}
