// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{EnvResolved, NeedsDetection};
use actingcommand_device::CaptureBackendConfig;
use actingcommand_recognition::Scene;
use serde::Serialize;
use std::path::PathBuf;
use std::time::Duration;

pub struct ReadonlyRecognitionInput {
    pub pack_path: PathBuf,
    pub pack_root: PathBuf,
    pub pages_path: Option<PathBuf>,
    pub marker_request: crate::EnvMarkerResolutionRequest,
    pub scene: Option<Scene>,
    pub scene_path: Option<PathBuf>,
    pub capture_config: Option<CaptureBackendConfig>,
    pub require_fresh: bool,
    pub fresh_delay: Duration,
}

pub struct RecognizeRequest {
    pub input: ReadonlyRecognitionInput,
    pub target: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum RecognizeResponse {
    ClickOnly(RecognizeClickOnlyResponse),
    Evaluated(Box<RecognizeEvaluatedResponse>),
}

#[derive(Debug, Clone, Serialize)]
pub struct RecognizeClickOnlyResponse {
    pub target: String,
    pub kind: String,
    pub evaluated: bool,
    pub click: RectResponse,
    pub match_metric: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub env_resolved: Vec<EnvResolved>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecognizeEvaluatedResponse {
    pub target: String,
    pub passed: bool,
    pub message: String,
    pub matched_rect: Option<RectResponse>,
    pub template: Option<TemplateEvaluationResponse>,
    pub color: Option<ColorEvaluationResponse>,
    pub evaluation: TargetEvaluationResponse,
    pub match_metric: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub env_resolved: Vec<EnvResolved>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub needs_detection: Option<NeedsDetection>,
}

pub struct DetectPageRequest {
    pub input: ReadonlyRecognitionInput,
    pub check_pages: bool,
}

pub struct DetectPageOutput {
    pub response: DetectPageResponse,
    pub env_resolved: Vec<EnvResolved>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum DetectPageResponse {
    Check(DetectPageCheckResponse),
    Detection(Box<PageDetectionResponse>),
}

#[derive(Debug, Clone, Serialize)]
pub struct DetectPageCheckResponse {
    pub check_pages: String,
}

pub struct CurrentPageRequest {
    pub input: ReadonlyRecognitionInput,
}

pub struct IsVisibleRequest {
    pub input: ReadonlyRecognitionInput,
    pub target: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct IsVisibleResponse {
    pub target: String,
    pub visible: bool,
    pub evaluation: TargetEvaluationResponse,
    pub match_metric: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub env_resolved: Vec<EnvResolved>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub needs_detection: Option<NeedsDetection>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PageDetectionResponse {
    pub page: String,
    pub matched: bool,
    pub standby: bool,
    pub evaluations: Vec<PageEvaluationResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_hint: Option<RecoveryHintResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub req_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reco_id: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub env_resolved: Vec<EnvResolved>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub needs_detection: Option<NeedsDetection>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecoveryHintResponse {
    pub action: String,
    pub point: PointResponse,
    pub note: String,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct PointResponse {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct RectResponse {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, Serialize)]
pub struct TargetEvaluationResponse {
    pub target: String,
    pub kind: String,
    pub passed: bool,
    pub message: String,
    pub matched_rect: Option<RectResponse>,
    pub template: Option<TemplateEvaluationResponse>,
    pub color: Option<ColorEvaluationResponse>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct TemplateEvaluationResponse {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub score: f32,
    pub raw_score: f32,
    pub threshold: f32,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ColorEvaluationResponse {
    pub distance: f32,
    pub max_distance: f32,
    pub mean: [u8; 3],
    pub expected: [u8; 3],
}

#[derive(Debug, Clone, Serialize)]
pub struct PageEvaluationResponse {
    pub page: String,
    pub matched: bool,
    pub message: String,
    pub any_of_passed: usize,
    pub any_of_total: usize,
    pub targets: Vec<PageTargetEvaluationResponse>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PageTargetEvaluationResponse {
    pub id: String,
    pub role: String,
    pub passed: bool,
    pub message: String,
}
