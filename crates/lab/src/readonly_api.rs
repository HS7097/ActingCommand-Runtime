// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_device::CaptureBackendConfig;
use actingcommand_recognition::Scene;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub use actingcommand_execution_kernel::{
    ColorEvaluationResponse, DetectPageCheckResponse, DetectPageOutput, DetectPageResponse,
    IsVisibleResponse, PageDetectionResponse, PageEvaluationResponse, PageTargetEvaluationResponse,
    PointResponse, RecognizeClickOnlyResponse, RecognizeEvaluatedResponse, RecognizeResponse,
    RecoveryHintResponse, RectResponse, TargetEvaluationResponse, TemplateEvaluationResponse,
};

pub struct ReadonlyRecognitionInput {
    pub resources: Arc<crate::ExternallyVerifiedBundle>,
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

pub struct DetectPageRequest {
    pub input: ReadonlyRecognitionInput,
    pub check_pages: bool,
}

pub struct CurrentPageRequest {
    pub input: ReadonlyRecognitionInput,
}

pub struct IsVisibleRequest {
    pub input: ReadonlyRecognitionInput,
    pub target: String,
}
