// SPDX-License-Identifier: AGPL-3.0-only

use crate::{FrameStoreControl, LabError, MemorySampleSource};
use actingcommand_device::{CaptureBackendChoice, CaptureBackendConfig, TouchBackendConfig};
use actingcommand_execution_kernel::Sha256Hash;
pub use actingcommand_execution_kernel::{ExternalExpectedSha256, ExternallyVerifiedBundle};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;

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

pub struct LabRunRequest {
    pub zip_path: PathBuf,
    pub out_path: PathBuf,
    pub run_root: PathBuf,
    pub game: Option<String>,
    pub server: Option<String>,
    pub instance: Option<String>,
    pub device_resolver: Box<dyn LabRunDeviceResolver>,
    pub capture_interval_override: Option<u64>,
    pub capture_backend_override: Option<CaptureBackendChoice>,
    pub frame_store_override: FrameStoreControl,
    pub expected_input_sha256: ExternalExpectedSha256,
    pub process: LabRunProcessContext,
}

pub trait LabRunDeviceResolver {
    fn resolve_selected(&mut self, instance_id: &str) -> Result<LabRunSelectedDevice, LabError>;
}

#[derive(Debug, Clone)]
pub struct LabRunSelectedDevice {
    id: String,
    serial: String,
    adb_provenance: String,
    capture_config: CaptureBackendConfig,
    touch_config: TouchBackendConfig,
}

impl LabRunSelectedDevice {
    pub fn new(
        id: impl Into<String>,
        serial: impl Into<String>,
        adb_provenance: impl Into<String>,
        capture_config: CaptureBackendConfig,
        touch_config: TouchBackendConfig,
    ) -> Self {
        Self {
            id: id.into(),
            serial: serial.into(),
            adb_provenance: adb_provenance.into(),
            capture_config,
            touch_config,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn serial(&self) -> &str {
        &self.serial
    }

    pub fn adb_provenance(&self) -> &str {
        &self.adb_provenance
    }

    pub fn capture_config(&self) -> &CaptureBackendConfig {
        &self.capture_config
    }

    pub fn touch_config(&self) -> &TouchBackendConfig {
        &self.touch_config
    }
}

pub trait RuntimeCommitSource: Send + Sync {
    fn sample(&self) -> Option<String>;
}

#[derive(Clone)]
pub struct LabRunProcessContext {
    pub current_dir: Option<PathBuf>,
    pub os: String,
    pub app_version: String,
    pub runtime_commit_source: Arc<dyn RuntimeCommitSource>,
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
