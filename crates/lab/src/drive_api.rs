// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{EnvResolved, InputSamplingEvidence, LabResult};
use actingcommand_execution_kernel::DriveDraw;
use serde::Serialize;
use std::time::Duration;

pub struct TapTargetRequest {
    pub input: crate::ReadonlyRecognitionInput,
    pub target: String,
    pub allow_destructive: bool,
    pub dry_run: bool,
    pub capture_requested: bool,
    /// Workflow #406 T4: how the point pressed in the target's click box is drawn; `None` presses
    /// the box's centre.
    pub draw: Option<DriveDraw>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TapTargetResponse {
    pub status: String,
    pub executed: bool,
    pub target: String,
    pub req_id: String,
    pub reco_id: String,
    pub action_id: String,
    pub click: crate::RectResponse,
    pub point: crate::PointResponse,
    /// Workflow #406 T4: how `point` was drawn, when the command was given a seed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampling: Option<InputSamplingEvidence>,
    pub evaluation: crate::TargetEvaluationResponse,
    pub safety_gate: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<SemanticDeviceResponse>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub env_resolved: Vec<EnvResolved>,
}

pub struct NavigateRequest {
    pub input: crate::ReadonlyRecognitionInput,
    pub to: String,
    pub allow_destructive: bool,
    pub dry_run: bool,
    pub capture_requested: bool,
    pub step_timeout: Option<LabResult<Duration>>,
    pub poll: Option<LabResult<Duration>>,
    /// Workflow #406 T4: how the route draws the point of each box it presses; `None` presses
    /// each box's centre.
    pub draw: Option<DriveDraw>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NavigateResponse {
    pub status: String,
    pub executed: bool,
    pub req_id: String,
    pub reco_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    pub to: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route: Option<Vec<NavigationEdgeResponse>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steps: Option<Vec<NavigationStepResponse>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safety_gate: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub env_resolved: Vec<EnvResolved>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NavigationEdgeResponse {
    pub id: String,
    pub from_page: String,
    pub to_page: String,
    pub input: SemanticInputResponse,
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NavigationStepResponse {
    pub action_id: String,
    pub edge: NavigationEdgeResponse,
    pub resolved_input: SemanticInputResponse,
    /// Workflow #406 T4: how the pressed points were drawn, when the route was given a seed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampling: Option<InputSamplingEvidence>,
    pub recognition: Option<NavigationTargetRecognitionResponse>,
    pub device: SemanticDeviceResponse,
    pub arrived: crate::PageDetectionResponse,
}

#[derive(Debug, Clone, Serialize)]
pub struct NavigationTargetRecognitionResponse {
    pub target_id: String,
    pub evaluation: crate::TargetEvaluationResponse,
}

#[derive(Debug, Clone, Serialize)]
pub struct SemanticDeviceResponse {
    #[serde(flatten)]
    pub report: crate::InputBackendReport,
    pub control_mode: String,
    pub action: SemanticInputResponse,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum SemanticInputResponse {
    #[serde(rename = "tap")]
    Tap {
        rect: crate::RectResponse,
        point: crate::PointResponse,
    },
    #[serde(rename = "target_center")]
    TargetCenter { target_id: String },
    /// Workflow #390 LAB-2 item 7: an `offset` click before its target is matched.
    #[serde(rename = "target_offset")]
    TargetOffset {
        target_id: String,
        offset: crate::RectResponse,
    },
    #[serde(rename = "drag")]
    Drag {
        from_rect: crate::RectResponse,
        to_rect: crate::RectResponse,
        from: crate::PointResponse,
        to: crate::PointResponse,
        duration_ms: u64,
    },
}
