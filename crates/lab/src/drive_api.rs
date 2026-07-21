// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{EnvResolved, LabResult};
use serde::Serialize;
use std::time::Duration;

pub struct TapTargetRequest {
    pub input: crate::ReadonlyRecognitionInput,
    pub target: String,
    pub allow_destructive: bool,
    pub dry_run: bool,
    pub capture_requested: bool,
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
    #[serde(rename = "target")]
    TargetTap {
        target_id: String,
        mode: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        offset: Option<crate::RectResponse>,
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
