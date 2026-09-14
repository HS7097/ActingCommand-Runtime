// SPDX-License-Identifier: AGPL-3.0-only

pub use actingcommand_execution_kernel::{ExternalExpectedSha256, ExternallyVerifiedBundle};
use actingcommand_pack_containment::Sha256Hash;
use serde::Serialize;
use std::path::PathBuf;

pub struct LabValidateRequest {
    pub zip_path: PathBuf,
    pub expected_input_sha256: Option<Sha256Hash>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LabContainedPackageValidationResponse {
    pub validation: LabValidateResponse,
    pub task_count: usize,
    pub entries: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LabValidateResponse {
    pub zip: String,
    pub status: String,
    pub input_sha256: String,
    pub hash_source: String,
    pub externally_verified: bool,
    pub entry_count: usize,
    pub control: LabValidateControlResponse,
    pub resources: LabValidateResourcesResponse,
}

#[derive(Debug, Clone, Serialize)]
pub struct LabValidateControlResponse {
    pub package_id: String,
    pub execution_mode: String,
    pub game: String,
    pub server: String,
    pub resolution: LabRunResolution,
    pub entry_task_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LabValidateResourcesResponse {
    pub resource_root: String,
    pub manifest: String,
    pub operation: String,
    pub operation_count: usize,
    pub pack: String,
    pub recognition_unsupported_target_count: usize,
    pub recognition_unsupported_targets: Vec<LabUnsupportedTargetResponse>,
    pub pages: String,
    pub navigation: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LabUnsupportedTargetResponse {
    pub id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct LabRunResolution {
    pub width: u32,
    pub height: u32,
}
