// SPDX-License-Identifier: AGPL-3.0-only

use crate::{FrameStoreControl, LabError, MemorySampleSource};
use actingcommand_device::{CaptureBackendChoice, CaptureBackendConfig, TouchBackendConfig};
use actingcommand_pack_containment::Sha256Hash;
use serde::Serialize;
use std::path::PathBuf;

pub struct LabValidateRequest {
    pub zip_path: PathBuf,
    pub expected_input_sha256: Option<Sha256Hash>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LabValidateResponse {
    pub zip: String,
    pub status: String,
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

pub struct LabRunRequest {
    pub zip_path: PathBuf,
    pub out_path: PathBuf,
    pub run_root: PathBuf,
    pub game: Option<String>,
    pub server: Option<String>,
    pub instance: Option<String>,
    pub device_candidates: Vec<LabRunDeviceCandidate>,
    pub capture_interval_override: Option<u64>,
    pub capture_backend_override: Option<CaptureBackendChoice>,
    pub frame_store_override: FrameStoreControl,
    pub expected_input_sha256: Option<Sha256Hash>,
    pub process: LabRunProcessContext,
}

pub struct LabRunDeviceCandidate {
    id: String,
    resolution: Result<LabRunDeviceConfig, LabError>,
}

impl LabRunDeviceCandidate {
    pub fn resolved(id: impl Into<String>, device: LabRunDeviceConfig) -> Self {
        Self {
            id: id.into(),
            resolution: Ok(device),
        }
    }

    pub fn failed(id: impl Into<String>, error: LabError) -> Self {
        Self {
            id: id.into(),
            resolution: Err(error),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn resolve(&self) -> Result<&LabRunDeviceConfig, LabError> {
        self.resolution.as_ref().map_err(Clone::clone)
    }
}

pub struct LabRunDeviceConfig {
    pub instance: String,
    pub adb_path: String,
    pub capture_config: CaptureBackendConfig,
    pub touch_config: TouchBackendConfig,
}

#[derive(Debug, Clone)]
pub struct LabRunProcessContext {
    pub current_dir: Option<PathBuf>,
    pub lease_root: PathBuf,
    pub os: String,
    pub runtime_commit: Option<String>,
    pub memory_source: MemorySampleSource,
}

#[derive(Debug, Clone, Serialize)]
pub struct LabRunResponse {
    pub ok: bool,
    pub status: String,
    pub run_id: String,
    pub result_zip: String,
    pub run_dir: String,
    pub run_dir_cleaned: bool,
    pub out: String,
    pub output_zip_sha256: String,
    pub ledger: LabRunLedgerResponse,
    pub screenshot_count: usize,
    pub executed_step_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct LabRunLedgerResponse {
    pub projection_source: String,
    pub path: String,
    pub terminal_receipt: String,
}
