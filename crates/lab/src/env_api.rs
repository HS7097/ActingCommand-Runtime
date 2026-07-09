// SPDX-License-Identifier: AGPL-3.0-only

use crate::{EnvDetectionResult, InputBackendReport};
use actingcommand_contract::EnvResolved;
use actingcommand_device::{CaptureBackendConfig, TouchBackendConfig};
use serde::Serialize;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct EnvScopeRequest {
    pub resource_root: PathBuf,
    pub state_root: PathBuf,
    pub instance: String,
    pub game: String,
    pub server: Option<String>,
}

#[derive(Debug, Clone)]
pub struct EnvDetectRequest {
    pub scope: EnvScopeRequest,
    pub task: String,
    pub scene_path: Option<PathBuf>,
    pub capture_config: Option<CaptureBackendConfig>,
    pub touch_config: Option<TouchBackendConfig>,
    pub require_fresh: bool,
    pub fresh_delay: Duration,
    pub dry_run: bool,
}

#[derive(Debug, Clone)]
pub struct EnvResolveRequest {
    pub scope: EnvScopeRequest,
    pub task: String,
    pub input: Option<String>,
    pub key: Option<String>,
}

#[derive(Debug, Clone)]
pub struct EnvStatusRequest {
    pub scope: EnvScopeRequest,
    pub task: String,
}

#[derive(Debug, Clone)]
pub struct EnvMarkerResolutionRequest {
    pub resource_root: PathBuf,
    pub instance: Option<String>,
    pub game: Option<String>,
    pub server: Option<String>,
    pub env_task: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EnvDetectResponse {
    pub schema_version: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dry_run: Option<bool>,
    pub task: String,
    pub detector_id: String,
    pub detector_version: String,
    pub instance_id: String,
    pub game_id: String,
    pub server_id: String,
    pub resource_root: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_path: Option<String>,
    pub steps_executed: bool,
    pub steps: Vec<EnvDetectionStepReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<EnvDetectionResult>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EnvDetectionStepReport {
    pub index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    pub step: EnvDetectionStepPlan,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<EnvTouchResult>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum EnvDetectionStepPlan {
    #[serde(rename = "tap")]
    Tap { x: i32, y: i32 },
    #[serde(rename = "long_tap")]
    LongTap { x: i32, y: i32, duration_ms: u64 },
    #[serde(rename = "swipe")]
    Swipe {
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
        duration_ms: u64,
    },
    #[serde(rename = "wait")]
    Wait { duration_ms: u64 },
}

#[derive(Debug, Clone, Serialize)]
pub struct EnvTouchResult {
    pub status: String,
    #[serde(flatten)]
    pub backend: InputBackendReport,
    pub control_mode: String,
    pub safety_gate: String,
    pub action: EnvTouchAction,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum EnvTouchAction {
    #[serde(rename = "tap")]
    Tap { x: i32, y: i32 },
    #[serde(rename = "long-tap")]
    LongTap { x: i32, y: i32, duration_ms: u64 },
    #[serde(rename = "swipe")]
    Swipe {
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
        duration_ms: u64,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct EnvResolveResponse {
    pub schema_version: String,
    pub status: String,
    pub task: String,
    pub detector_id: String,
    pub instance_id: String,
    pub source_result: String,
    pub resolved: String,
    pub keys: Vec<EnvResolved>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EnvStatusResponse {
    pub schema_version: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub task: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detector_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detector_version: Option<String>,
    pub instance_id: String,
    pub result_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<EnvDetectionResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub needs_detection: Option<EnvNeedsDetectionPayload>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EnvNeedsDetectionPayload {
    pub status: String,
    pub reason: String,
    pub task: String,
    pub detector_id: String,
    pub detector_version: String,
    pub instance_id: String,
    pub game_id: String,
    pub server_id: String,
    pub result_path: String,
    pub recommended_action: String,
    pub detections: Vec<EnvResolved>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_generated_at_unix_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_resource_pack_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
