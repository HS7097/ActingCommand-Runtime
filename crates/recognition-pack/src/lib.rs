// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_recognition as recognition;
use recognition::{MatchMetric, Scene};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub type RecognitionPackResult<T> = Result<T, RecognitionPackError>;

const MAX_VISION_TIMEOUT_MS: u64 = 60_000;
const MAX_VISION_LANGUAGES: usize = 8;
const MAX_VISION_EXPECTED_VALUES: usize = 64;
const MAX_VISION_LABELS: usize = 256;
const MAX_VISION_STRING_BYTES: usize = 4_096;
const MAX_OCR_TEXT_BYTES: usize = 64 * 1024;
const MAX_OCR_BLOCKS: usize = 1_024;
const MAX_VISION_RESULTS: usize = 1_024;
const MAX_TEMPLATE_REGION_EVALUATIONS: usize = 64;
const PPOCR_V6_MEDIUM_MODEL_REF: &str = "PP-OCRv6_medium";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecognitionPackErrorSeverity {
    Fatal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecognitionPackErrorCode {
    InvalidPackage,
    UnsupportedTarget,
    VisionProviderMissing,
    VisionProviderFailure,
    VisionProviderInvalidResponse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecognitionPackError {
    severity: RecognitionPackErrorSeverity,
    code: RecognitionPackErrorCode,
    message: String,
}

impl RecognitionPackError {
    pub fn fatal(message: impl Into<String>) -> Self {
        Self::fatal_with_code(RecognitionPackErrorCode::InvalidPackage, message)
    }

    pub fn fatal_with_code(code: RecognitionPackErrorCode, message: impl Into<String>) -> Self {
        Self {
            severity: RecognitionPackErrorSeverity::Fatal,
            code,
            message: message.into(),
        }
    }

    pub fn severity(&self) -> RecognitionPackErrorSeverity {
        self.severity
    }

    pub fn code(&self) -> RecognitionPackErrorCode {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for RecognitionPackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.severity {
            RecognitionPackErrorSeverity::Fatal => {
                write!(f, "fatal recognition pack error: {}", self.message)
            }
        }
    }
}

impl Error for RecognitionPackError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl From<PackRect> for recognition::Rect {
    fn from(rect: PackRect) -> Self {
        recognition::Rect {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackPoint {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PackRegion {
    Rect(PackRect),
    Keyword(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackCoordinateSpace {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RecognitionPack {
    pub schema_version: String,
    pub game: Option<String>,
    pub server: Option<String>,
    pub locale: Option<String>,
    pub coordinate_space: Option<PackCoordinateSpace>,
    #[serde(default)]
    pub defaults: RecognitionDefaults,
    pub targets: Vec<RecognitionTarget>,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct RecognitionDefaults {
    #[serde(default = "default_template_threshold")]
    pub template_threshold: f32,
    #[serde(default = "default_color_max_distance")]
    pub color_max_distance: f32,
    #[serde(default = "default_match_metric")]
    pub match_metric: RecognitionMatchMetric,
}

impl Default for RecognitionDefaults {
    fn default() -> Self {
        Self {
            template_threshold: default_template_threshold(),
            color_max_distance: default_color_max_distance(),
            match_metric: default_match_metric(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecognitionMethod {
    #[default]
    Ncc,
    RgbCount,
    HsvCount,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RecognitionMask {
    Range { lower: u8, upper: u8 },
    Bitmap { path: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecognitionMatchMetric {
    CcorrNormed,
    CcoeffNormed,
}

impl RecognitionMatchMetric {
    fn as_match_metric(self) -> MatchMetric {
        match self {
            Self::CcorrNormed => MatchMetric::CrossCorrelationNormalized,
            Self::CcoeffNormed => MatchMetric::CorrelationCoefficientNormalized,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RecognitionTarget {
    Template(TemplateTarget),
    Color(ColorTarget),
    ClickOnly(ClickOnlyTarget),
    Ocr(OcrTarget),
    Nn(NnTarget),
}

#[derive(Debug, Clone, Deserialize)]
pub struct TemplateTarget {
    pub id: String,
    pub template_path: String,
    pub region: PackRegion,
    #[serde(default)]
    pub threshold: Option<f32>,
    #[serde(default)]
    pub method: RecognitionMethod,
    pub mask: Option<RecognitionMask>,
    pub rect_move: Option<PackRect>,
    pub color_check: Option<ColorCheck>,
    pub click: Option<PackRect>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ColorTarget {
    pub id: String,
    pub region: PackRect,
    pub expected: [u8; 3],
    pub click: Option<PackRect>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ClickOnlyTarget {
    pub id: String,
    pub click: PackRect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrMatchMode {
    Exact,
    Contains,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OcrTarget {
    pub id: String,
    pub region: PackRegion,
    pub languages: Vec<String>,
    pub timeout_ms: u64,
    pub match_mode: OcrMatchMode,
    pub expected: Vec<String>,
    pub case_sensitive: bool,
    pub minimum_confidence: f32,
    pub model_ref: String,
    pub model_sha256: String,
    pub click: Option<PackRect>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NnSelectionMode {
    Best,
    Label,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NnTarget {
    pub id: String,
    pub region: PackRegion,
    pub model_ref: String,
    pub model_sha256: String,
    pub candidate_labels: Vec<String>,
    pub minimum_score: f32,
    pub selection: NnSelectionMode,
    pub expected_label: Option<String>,
    pub timeout_ms: u64,
    pub click: Option<PackRect>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct ColorCheck {
    pub region: PackRect,
    pub expected: [u8; 3],
}

#[derive(Debug, Clone)]
pub struct RecognitionEvaluator {
    asset_resolver: Arc<dyn AssetResolver>,
    vision_provider: Option<Arc<dyn VisionProvider>>,
    pack: RecognitionPack,
    target_indexes: HashMap<String, usize>,
    unsupported_targets: Vec<UnsupportedRecognitionTarget>,
}

#[derive(Debug, Clone, Copy)]
pub struct VisionProviderFrame<'a> {
    pub width: u32,
    pub height: u32,
    pub rgb8_pixels: &'a [u8],
}

#[derive(Debug, Clone, Copy)]
pub struct OcrProviderRequest<'a> {
    pub frame: VisionProviderFrame<'a>,
    pub region: PackRect,
    pub languages: &'a [String],
    pub timeout_ms: u64,
    pub model_ref: &'a str,
    pub model_sha256: &'a str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OcrProviderTextBlock {
    pub text: String,
    pub rect: PackRect,
    pub confidence: Option<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OcrProviderResult {
    pub text: String,
    pub blocks: Vec<OcrProviderTextBlock>,
    pub confidence: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrExecutionProviderKind {
    Cpu,
    Cuda,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OcrProviderExecutionEvidence {
    pub invocation_id: String,
    pub session_id: String,
    pub session_generation: u64,
    pub requested_provider: OcrExecutionProviderKind,
    pub resolved_provider: OcrExecutionProviderKind,
    pub requested_cuda_ordinal: Option<u32>,
    pub requested_cuda_identity: Option<String>,
    pub resolved_cuda_ordinal: Option<u32>,
    pub resolved_cuda_identity: Option<String>,
    pub provider_implementation: String,
    pub provider_binary_sha256: String,
    pub runtime_version: String,
    pub model_ref: String,
    pub model_sha256: String,
    pub cpu_ep_registered: bool,
    pub cpu_fallback_disabled: bool,
    pub fallback_forbidden: bool,
    pub fallback_observed: Option<bool>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OcrProviderObservation {
    pub result: OcrProviderResult,
    pub execution: Option<OcrProviderExecutionEvidence>,
}

#[derive(Debug, Clone, Copy)]
pub struct NnProviderRequest<'a> {
    pub frame: VisionProviderFrame<'a>,
    pub region: PackRect,
    pub model_ref: &'a str,
    pub model_sha256: &'a str,
    pub candidate_labels: &'a [String],
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NnProviderLabel {
    pub label: String,
    pub score: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NnProviderResult {
    pub labels: Vec<NnProviderLabel>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisionProviderErrorCode {
    Unavailable,
    Timeout,
    ModelMismatch,
    InvalidResponse,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisionProviderError {
    code: VisionProviderErrorCode,
    message: String,
}

impl VisionProviderError {
    pub fn new(code: VisionProviderErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn code(&self) -> VisionProviderErrorCode {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for VisionProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "vision provider error ({:?}): {}",
            self.code, self.message
        )
    }
}

impl Error for VisionProviderError {}

pub trait VisionProvider: fmt::Debug + Send + Sync {
    fn require_ocr_model(
        &self,
        model_ref: &str,
        model_sha256: &str,
    ) -> Result<(), VisionProviderError>;

    fn require_nn_model(
        &self,
        model_ref: &str,
        model_sha256: &str,
    ) -> Result<(), VisionProviderError>;

    fn read_text(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderResult, VisionProviderError>;

    fn read_text_with_execution_evidence(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderObservation, VisionProviderError> {
        self.read_text(request)
            .map(|result| OcrProviderObservation {
                result,
                execution: None,
            })
    }

    fn classify(
        &self,
        request: NnProviderRequest<'_>,
    ) -> Result<NnProviderResult, VisionProviderError>;
}

pub trait AssetResolver: fmt::Debug + Send + Sync {
    fn read_asset(&self, path: &str) -> RecognitionPackResult<Vec<u8>>;

    fn contains_asset(&self, path: &str) -> bool {
        self.read_asset(path).is_ok()
    }
}

#[derive(Debug, Clone)]
pub struct FsAssetResolver {
    root: PathBuf,
}

impl FsAssetResolver {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl AssetResolver for FsAssetResolver {
    fn read_asset(&self, path: &str) -> RecognitionPackResult<Vec<u8>> {
        fs::read(self.root.join(path)).map_err(|err| {
            RecognitionPackError::fatal(format!("failed to read asset '{path}': {err}"))
        })
    }

    fn contains_asset(&self, path: &str) -> bool {
        self.root.join(path).is_file()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    Template,
    Color,
    ClickOnly,
    Ocr,
    Nn,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TargetEvaluation {
    pub id: String,
    pub kind: TargetKind,
    pub passed: bool,
    pub template: Option<TemplateEvaluation>,
    pub color: Option<ColorEvaluation>,
    pub ocr: Option<OcrEvaluation>,
    pub nn: Option<NnEvaluation>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct TemplateEvaluation {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub raw_score: f32,
    pub score: f32,
    pub threshold: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TemplateRegionEvaluationBatch {
    pub target_id: String,
    pub rows: Vec<TemplateRegionEvaluationRow>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct TemplateRegionEvaluationRow {
    pub index: usize,
    pub requested_region: PackRect,
    pub metric: RecognitionMatchMetric,
    pub matched_rect: PackRect,
    pub raw_score: f32,
    pub normalized_score: f32,
    pub threshold: f32,
    pub passed: bool,
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedRecognitionTarget {
    pub id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ColorEvaluation {
    pub distance: f32,
    pub max_distance: f32,
    pub mean: [u8; 3],
    pub expected: [u8; 3],
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OcrEvaluation {
    pub text: String,
    pub confidence: Option<f32>,
    pub matched_expected: Option<String>,
    pub match_mode: OcrMatchMode,
    pub blocks: Vec<OcrTextEvidence>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OcrTextEvidence {
    pub text: String,
    pub rect: PackRect,
    pub confidence: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OcrObservationEvaluation {
    pub target_id: String,
    pub text: String,
    pub confidence: Option<f32>,
    pub blocks: Vec<OcrTextEvidence>,
    pub execution: OcrProviderExecutionEvidence,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NnEvaluation {
    pub selected_label: Option<String>,
    pub selected_score: Option<f32>,
    pub selection: NnSelectionMode,
    pub labels: Vec<NnLabelEvidence>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NnLabelEvidence {
    pub label: String,
    pub score: f32,
    pub candidate: bool,
}

pub fn load_pack_from_json_str(json: &str) -> RecognitionPackResult<RecognitionPack> {
    let value: Value = serde_json::from_str(json).map_err(|err| {
        RecognitionPackError::fatal(format!("failed to parse recognition pack JSON: {err}"))
    })?;
    let schema_version = value
        .as_object()
        .and_then(|object| object.get("schema_version"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            RecognitionPackError::fatal(
                "failed to parse recognition pack JSON: schema_version must be a string",
            )
        })?
        .to_string();

    if schema_version == "0.6" {
        validate_v06_wire_shape(&value)?;
    }

    let pack: RecognitionPack = serde_json::from_value(value).map_err(|err| {
        RecognitionPackError::fatal(format!("failed to parse recognition pack JSON: {err}"))
    })?;
    if matches!(schema_version.as_str(), "0.1" | "0.3" | "0.4" | "0.5")
        && pack
            .targets
            .iter()
            .any(|target| matches!(target, RecognitionTarget::Ocr(_) | RecognitionTarget::Nn(_)))
    {
        return Err(RecognitionPackError::fatal_with_code(
            RecognitionPackErrorCode::UnsupportedTarget,
            format!(
                "recognition target type ocr/nn requires schema_version '0.6', got '{schema_version}'"
            ),
        ));
    }
    Ok(pack)
}

impl RecognitionEvaluator {
    pub fn new(pack_root: PathBuf, pack: RecognitionPack) -> RecognitionPackResult<Self> {
        Self::with_asset_resolver(pack, Arc::new(FsAssetResolver::new(pack_root)))
    }

    pub fn with_asset_resolver(
        pack: RecognitionPack,
        asset_resolver: Arc<dyn AssetResolver>,
    ) -> RecognitionPackResult<Self> {
        Self::with_optional_vision_provider(pack, asset_resolver, None)
    }

    pub fn with_vision_provider(
        pack: RecognitionPack,
        asset_resolver: Arc<dyn AssetResolver>,
        vision_provider: Arc<dyn VisionProvider>,
    ) -> RecognitionPackResult<Self> {
        Self::with_optional_vision_provider(pack, asset_resolver, Some(vision_provider))
    }

    fn with_optional_vision_provider(
        pack: RecognitionPack,
        asset_resolver: Arc<dyn AssetResolver>,
        vision_provider: Option<Arc<dyn VisionProvider>>,
    ) -> RecognitionPackResult<Self> {
        let mut errors = Vec::new();
        validate_pack(asset_resolver.as_ref(), &pack, &mut errors);
        if !errors.is_empty() {
            return Err(RecognitionPackError::fatal(errors.join("; ")));
        }

        let target_indexes = pack
            .targets
            .iter()
            .enumerate()
            .map(|(index, target)| (target.id().to_string(), index))
            .collect();
        let unsupported_targets = unsupported_recognition_targets(&pack);

        Ok(Self {
            asset_resolver,
            vision_provider,
            pack,
            target_indexes,
            unsupported_targets,
        })
    }

    pub fn pack(&self) -> &RecognitionPack {
        &self.pack
    }

    pub fn evaluate_target(
        &self,
        scene: &Scene,
        target_id: &str,
    ) -> RecognitionPackResult<TargetEvaluation> {
        self.validate_coordinate_space(scene)?;
        let target = self.target(target_id)?;

        match target {
            RecognitionTarget::Template(target) => {
                if let Some(reason) = unsupported_template_reason(target) {
                    return Err(RecognitionPackError::fatal(format!(
                        "template target '{}' uses unsupported recognition semantics: {reason}",
                        target.id
                    )));
                }
                self.evaluate_template(scene, target)
            }
            RecognitionTarget::Color(target) => self.evaluate_color(scene, target),
            RecognitionTarget::ClickOnly(target) => Err(RecognitionPackError::fatal(format!(
                "click-only target '{}' cannot be evaluated",
                target.id
            ))),
            RecognitionTarget::Ocr(target) => self.evaluate_ocr(scene, target),
            RecognitionTarget::Nn(target) => self.evaluate_nn(scene, target),
        }
    }

    pub fn evaluate_template_regions(
        &self,
        scene: &Scene,
        target_id: &str,
        regions: &[PackRect],
    ) -> RecognitionPackResult<TemplateRegionEvaluationBatch> {
        self.validate_coordinate_space(scene)?;
        let RecognitionTarget::Template(target) = self.target(target_id)? else {
            return Err(RecognitionPackError::fatal(format!(
                "recognition target '{target_id}' is not a template target"
            )));
        };
        if let Some(reason) = unsupported_template_reason(target) {
            return Err(RecognitionPackError::fatal(format!(
                "template target '{}' uses unsupported recognition semantics: {reason}",
                target.id
            )));
        }
        if !(1..=MAX_TEMPLATE_REGION_EVALUATIONS).contains(&regions.len()) {
            return Err(RecognitionPackError::fatal(format!(
                "template target '{}' region batch must contain 1..={MAX_TEMPLATE_REGION_EVALUATIONS} regions, got {}",
                target.id,
                regions.len()
            )));
        }

        let scene_bounds = PackRect {
            x: 0,
            y: 0,
            width: i32::try_from(scene.width())
                .map_err(|_| RecognitionPackError::fatal("scene width exceeds i32 range"))?,
            height: i32::try_from(scene.height())
                .map_err(|_| RecognitionPackError::fatal("scene height exceeds i32 range"))?,
        };
        for (index, region) in regions.iter().enumerate() {
            if !rect_is_within(*region, scene_bounds) {
                return Err(RecognitionPackError::fatal(format!(
                    "template target '{}' region[{index}] must be nonempty and within scene bounds",
                    target.id
                )));
            }
            if let Some(first_index) = regions[..index]
                .iter()
                .position(|candidate| candidate == region)
            {
                return Err(RecognitionPackError::fatal(format!(
                    "template target '{}' region[{index}] duplicates region[{first_index}]",
                    target.id
                )));
            }
        }

        let template_png = self
            .asset_resolver
            .read_asset(&target.template_path)
            .map_err(|err| {
                RecognitionPackError::fatal(format!(
                    "failed to read template '{}' for target '{}': {}",
                    target.template_path,
                    target.id,
                    err.message()
                ))
            })?;
        let metric = self.pack.defaults.match_metric;
        let threshold = target
            .threshold
            .unwrap_or(self.pack.defaults.template_threshold);
        let mut rows = Vec::with_capacity(regions.len());
        for (index, requested_region) in regions.iter().copied().enumerate() {
            let matched = scene
                .match_template_with_metric(
                    &template_png,
                    Some(requested_region.into()),
                    metric.as_match_metric(),
                )
                .map_err(|err| primitive_error(&target.id, err))?;
            rows.push(TemplateRegionEvaluationRow {
                index,
                requested_region,
                metric,
                matched_rect: PackRect {
                    x: matched.x,
                    y: matched.y,
                    width: matched.width,
                    height: matched.height,
                },
                raw_score: matched.raw_score,
                normalized_score: matched.score,
                threshold,
                passed: matched.score >= threshold,
                selected: false,
            });
        }

        let mut selected_index: Option<usize> = None;
        for index in 0..rows.len() {
            if rows[index].passed
                && selected_index.is_none_or(|selected| {
                    rows[index].normalized_score > rows[selected].normalized_score
                })
            {
                selected_index = Some(index);
            }
        }
        if let Some(index) = selected_index {
            rows[index].selected = true;
        }

        Ok(TemplateRegionEvaluationBatch {
            target_id: target.id.clone(),
            rows,
        })
    }

    pub fn evaluate_ocr_observation(
        &self,
        scene: &Scene,
        target_id: &str,
    ) -> RecognitionPackResult<OcrObservationEvaluation> {
        self.validate_coordinate_space(scene)?;
        let RecognitionTarget::Ocr(target) = self.target(target_id)? else {
            return Err(RecognitionPackError::fatal(format!(
                "post-admission OCR target '{target_id}' is not an OCR target"
            )));
        };
        let provider = self.vision_provider.as_ref().ok_or_else(|| {
            RecognitionPackError::fatal_with_code(
                RecognitionPackErrorCode::VisionProviderMissing,
                format!("ocr target '{}' has no injected vision provider", target.id),
            )
        })?;
        let region = provider_region(scene, &target.id, &target.region)?;
        provider
            .require_ocr_model(&target.model_ref, &target.model_sha256)
            .map_err(|error| provider_error(&target.id, "ocr admission", error))?;
        let observation = provider
            .read_text_with_execution_evidence(OcrProviderRequest {
                frame: provider_frame(scene),
                region,
                languages: &target.languages,
                timeout_ms: target.timeout_ms,
                model_ref: &target.model_ref,
                model_sha256: &target.model_sha256,
            })
            .map_err(|err| provider_error(&target.id, "ocr observation", err))?;
        let execution = observation.execution.ok_or_else(|| {
            RecognitionPackError::fatal_with_code(
                RecognitionPackErrorCode::VisionProviderInvalidResponse,
                format!(
                    "ocr observation for target '{}' is missing execution evidence",
                    target.id
                ),
            )
        })?;
        if execution.model_ref != target.model_ref
            || execution.model_sha256 != target.model_sha256
            || !ocr_execution_provider_binding_is_valid(&execution)
            || !execution.complete
            || !execution.fallback_forbidden
            || execution.fallback_observed.is_some()
        {
            return Err(RecognitionPackError::fatal_with_code(
                RecognitionPackErrorCode::VisionProviderInvalidResponse,
                format!(
                    "ocr observation execution evidence does not match target '{}'",
                    target.id
                ),
            ));
        }
        let ocr = validate_ocr_result(observation.result, region)?;
        Ok(OcrObservationEvaluation {
            target_id: target.id.clone(),
            text: ocr.text,
            confidence: ocr.confidence,
            blocks: ocr.blocks,
            execution,
        })
    }

    pub fn get_click_target(&self, target_id: &str) -> RecognitionPackResult<PackRect> {
        let target = self.target(target_id)?;
        match target {
            RecognitionTarget::Template(target) => target.click.ok_or_else(|| {
                RecognitionPackError::fatal(format!(
                    "template target '{}' has no click field",
                    target.id
                ))
            }),
            RecognitionTarget::Color(target) => target.click.ok_or_else(|| {
                RecognitionPackError::fatal(format!(
                    "color target '{}' has no click field",
                    target.id
                ))
            }),
            RecognitionTarget::ClickOnly(target) => Ok(target.click),
            RecognitionTarget::Ocr(target) => target.click.ok_or_else(|| {
                RecognitionPackError::fatal(format!(
                    "ocr target '{}' has no click field",
                    target.id
                ))
            }),
            RecognitionTarget::Nn(target) => target.click.ok_or_else(|| {
                RecognitionPackError::fatal(format!("nn target '{}' has no click field", target.id))
            }),
        }
    }

    pub fn get_template_anchor_rect(
        &self,
        target_id: &str,
    ) -> RecognitionPackResult<Option<PackRect>> {
        match self.target(target_id)? {
            RecognitionTarget::Template(target) => match target.region {
                PackRegion::Rect(rect) => Ok(Some(rect)),
                PackRegion::Keyword(ref value) if value == "full_frame" => Ok(None),
                PackRegion::Keyword(ref value) => Err(RecognitionPackError::fatal(format!(
                    "template target '{}' has unsupported region '{value}'",
                    target.id
                ))),
            },
            RecognitionTarget::Color(_)
            | RecognitionTarget::ClickOnly(_)
            | RecognitionTarget::Ocr(_)
            | RecognitionTarget::Nn(_) => Ok(None),
        }
    }

    pub fn target_kind(&self, target_id: &str) -> RecognitionPackResult<TargetKind> {
        let target = self.target(target_id)?;
        Ok(match target {
            RecognitionTarget::Template(_) => TargetKind::Template,
            RecognitionTarget::Color(_) => TargetKind::Color,
            RecognitionTarget::ClickOnly(_) => TargetKind::ClickOnly,
            RecognitionTarget::Ocr(_) => TargetKind::Ocr,
            RecognitionTarget::Nn(_) => TargetKind::Nn,
        })
    }

    pub fn default_match_metric(&self) -> MatchMetric {
        self.pack.defaults.match_metric.as_match_metric()
    }

    pub fn unsupported_target_count(&self) -> usize {
        self.unsupported_targets.len()
    }

    pub fn unsupported_targets(&self) -> &[UnsupportedRecognitionTarget] {
        &self.unsupported_targets
    }

    fn evaluate_template(
        &self,
        scene: &Scene,
        target: &TemplateTarget,
    ) -> RecognitionPackResult<TargetEvaluation> {
        let template_png = self
            .asset_resolver
            .read_asset(&target.template_path)
            .map_err(|err| {
                RecognitionPackError::fatal(format!(
                    "failed to read template '{}' for target '{}': {}",
                    target.template_path,
                    target.id,
                    err.message()
                ))
            })?;
        let region = target_region(&target.id, &target.region)?;
        let matched = scene
            .match_template_with_metric(&template_png, region, self.default_match_metric())
            .map_err(|err| primitive_error(&target.id, err))?;
        let threshold = target
            .threshold
            .unwrap_or(self.pack.defaults.template_threshold);
        let template = TemplateEvaluation {
            x: matched.x,
            y: matched.y,
            width: matched.width,
            height: matched.height,
            raw_score: matched.raw_score,
            score: matched.score,
            threshold,
        };
        let template_ok = template.score >= template.threshold;

        let color = match target.color_check {
            Some(check) => Some(self.evaluate_color_check(scene, &target.id, check)?),
            None => None,
        };
        let color_ok = color
            .as_ref()
            .is_none_or(|color| color.distance <= color.max_distance);
        let passed = template_ok && color_ok;

        Ok(TargetEvaluation {
            id: target.id.clone(),
            kind: TargetKind::Template,
            passed,
            template: Some(template),
            color,
            ocr: None,
            nn: None,
            message: template_message(template_ok, color_ok),
        })
    }

    fn evaluate_color(
        &self,
        scene: &Scene,
        target: &ColorTarget,
    ) -> RecognitionPackResult<TargetEvaluation> {
        let color = self.evaluate_color_match(scene, &target.id, target.region, target.expected)?;
        let passed = color.distance <= color.max_distance;

        Ok(TargetEvaluation {
            id: target.id.clone(),
            kind: TargetKind::Color,
            passed,
            template: None,
            color: Some(color),
            ocr: None,
            nn: None,
            message: if passed {
                "color passed".to_string()
            } else {
                "color failed".to_string()
            },
        })
    }

    fn evaluate_ocr(
        &self,
        scene: &Scene,
        target: &OcrTarget,
    ) -> RecognitionPackResult<TargetEvaluation> {
        let provider = self.vision_provider.as_ref().ok_or_else(|| {
            RecognitionPackError::fatal_with_code(
                RecognitionPackErrorCode::VisionProviderMissing,
                format!("ocr target '{}' has no injected vision provider", target.id),
            )
        })?;
        let region = provider_region(scene, &target.id, &target.region)?;
        provider
            .require_ocr_model(&target.model_ref, &target.model_sha256)
            .map_err(|error| provider_error(&target.id, "ocr admission", error))?;
        let result = provider
            .read_text(OcrProviderRequest {
                frame: provider_frame(scene),
                region,
                languages: &target.languages,
                timeout_ms: target.timeout_ms,
                model_ref: &target.model_ref,
                model_sha256: &target.model_sha256,
            })
            .map_err(|err| provider_error(&target.id, "ocr", err))?;
        let ocr = validate_ocr_result(result, region)?;
        let matched_expected = target
            .expected
            .iter()
            .find(|expected| ocr_text_matches(&ocr.text, expected, target))
            .cloned();
        let confidence_ok = ocr
            .confidence
            .is_some_and(|confidence| confidence >= target.minimum_confidence);
        let passed = matched_expected.is_some() && confidence_ok;
        let message = match (matched_expected.is_some(), confidence_ok) {
            (true, true) => "ocr passed",
            (false, true) => "ocr text did not match",
            (true, false) => "ocr confidence below threshold",
            (false, false) => "ocr text did not match and confidence below threshold",
        }
        .to_string();

        Ok(TargetEvaluation {
            id: target.id.clone(),
            kind: TargetKind::Ocr,
            passed,
            template: None,
            color: None,
            ocr: Some(OcrEvaluation {
                text: ocr.text,
                confidence: ocr.confidence,
                matched_expected,
                match_mode: target.match_mode,
                blocks: ocr.blocks,
            }),
            nn: None,
            message,
        })
    }

    fn evaluate_nn(
        &self,
        scene: &Scene,
        target: &NnTarget,
    ) -> RecognitionPackResult<TargetEvaluation> {
        let provider = self.vision_provider.as_ref().ok_or_else(|| {
            RecognitionPackError::fatal_with_code(
                RecognitionPackErrorCode::VisionProviderMissing,
                format!("nn target '{}' has no injected vision provider", target.id),
            )
        })?;
        let region = provider_region(scene, &target.id, &target.region)?;
        provider
            .require_nn_model(&target.model_ref, &target.model_sha256)
            .map_err(|error| provider_error(&target.id, "nn admission", error))?;
        let result = provider
            .classify(NnProviderRequest {
                frame: provider_frame(scene),
                region,
                model_ref: &target.model_ref,
                model_sha256: &target.model_sha256,
                candidate_labels: &target.candidate_labels,
                timeout_ms: target.timeout_ms,
            })
            .map_err(|err| provider_error(&target.id, "nn", err))?;
        let labels = validate_nn_result(result, &target.candidate_labels)?;
        let selected = match target.selection {
            NnSelectionMode::Best => labels.iter().find(|label| label.candidate),
            NnSelectionMode::Label => target.expected_label.as_deref().and_then(|expected| {
                labels
                    .iter()
                    .find(|label| label.candidate && label.label == expected)
            }),
        };
        let selected_label = selected.map(|label| label.label.clone());
        let selected_score = selected.map(|label| label.score);
        let passed = selected_score.is_some_and(|score| score >= target.minimum_score);

        Ok(TargetEvaluation {
            id: target.id.clone(),
            kind: TargetKind::Nn,
            passed,
            template: None,
            color: None,
            ocr: None,
            nn: Some(NnEvaluation {
                selected_label,
                selected_score,
                selection: target.selection,
                labels,
            }),
            message: if passed {
                "nn passed".to_string()
            } else {
                "nn score below threshold or no eligible label".to_string()
            },
        })
    }

    fn evaluate_color_check(
        &self,
        scene: &Scene,
        target_id: &str,
        check: ColorCheck,
    ) -> RecognitionPackResult<ColorEvaluation> {
        self.evaluate_color_match(scene, target_id, check.region, check.expected)
    }

    fn evaluate_color_match(
        &self,
        scene: &Scene,
        target_id: &str,
        region: PackRect,
        expected: [u8; 3],
    ) -> RecognitionPackResult<ColorEvaluation> {
        let matched = scene
            .compare_color(region.into(), expected)
            .map_err(|err| primitive_error(target_id, err))?;
        Ok(ColorEvaluation {
            distance: matched.distance,
            max_distance: self.pack.defaults.color_max_distance,
            mean: matched.mean,
            expected,
        })
    }

    fn validate_coordinate_space(&self, scene: &Scene) -> RecognitionPackResult<()> {
        if let Some(expected) = self.pack.coordinate_space
            && (scene.width() != expected.width || scene.height() != expected.height)
        {
            return Err(RecognitionPackError::fatal(format!(
                "scene dimensions {}x{} do not match pack coordinate_space {}x{}",
                scene.width(),
                scene.height(),
                expected.width,
                expected.height
            )));
        }
        Ok(())
    }

    fn target(&self, target_id: &str) -> RecognitionPackResult<&RecognitionTarget> {
        let index = self.target_indexes.get(target_id).ok_or_else(|| {
            RecognitionPackError::fatal(format!("target id not found: {target_id}"))
        })?;
        Ok(&self.pack.targets[*index])
    }
}

pub fn unsupported_recognition_targets(
    pack: &RecognitionPack,
) -> Vec<UnsupportedRecognitionTarget> {
    pack.targets
        .iter()
        .filter_map(|target| match target {
            RecognitionTarget::Template(target) => {
                unsupported_template_reason(target).map(|reason| UnsupportedRecognitionTarget {
                    id: target.id.clone(),
                    reason,
                })
            }
            RecognitionTarget::Color(_)
            | RecognitionTarget::ClickOnly(_)
            | RecognitionTarget::Ocr(_)
            | RecognitionTarget::Nn(_) => None,
        })
        .collect()
}

impl RecognitionTarget {
    fn id(&self) -> &str {
        match self {
            Self::Template(target) => &target.id,
            Self::Color(target) => &target.id,
            Self::ClickOnly(target) => &target.id,
            Self::Ocr(target) => &target.id,
            Self::Nn(target) => &target.id,
        }
    }
}

fn validate_v06_wire_shape(value: &Value) -> RecognitionPackResult<()> {
    let root = value.as_object().ok_or_else(|| {
        RecognitionPackError::fatal("schema 0.6 recognition pack root must be an object")
    })?;
    reject_unknown_fields(
        root,
        &[
            "schema_version",
            "converter_schema_version",
            "generated",
            "generated_by",
            "game",
            "server",
            "locale",
            "coordinate_space",
            "defaults",
            "targets",
        ],
        "schema 0.6 recognition pack",
    )?;
    for field in ["converter_schema_version", "generated_by"] {
        if root.get(field).is_some_and(|value| !value.is_string()) {
            return Err(RecognitionPackError::fatal(format!(
                "schema 0.6 recognition pack field '{field}' must be a string"
            )));
        }
    }
    if root
        .get("generated")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err(RecognitionPackError::fatal(
            "schema 0.6 recognition pack field 'generated' must be a boolean",
        ));
    }
    if let Some(coordinate_space) = root.get("coordinate_space")
        && !coordinate_space.is_null()
    {
        validate_strict_object(
            coordinate_space,
            &["width", "height"],
            "schema 0.6 coordinate_space",
        )?;
    }
    if let Some(defaults) = root.get("defaults") {
        validate_strict_object(
            defaults,
            &["template_threshold", "color_max_distance", "match_metric"],
            "schema 0.6 defaults",
        )?;
    }
    let targets = root
        .get("targets")
        .and_then(Value::as_array)
        .ok_or_else(|| RecognitionPackError::fatal("schema 0.6 targets must be an array"))?;
    for (index, target) in targets.iter().enumerate() {
        let object = target.as_object().ok_or_else(|| {
            RecognitionPackError::fatal(format!("schema 0.6 target[{index}] must be an object"))
        })?;
        let target_type = object.get("type").and_then(Value::as_str).ok_or_else(|| {
            RecognitionPackError::fatal(format!("schema 0.6 target[{index}].type must be a string"))
        })?;
        let allowed = match target_type {
            "template" => {
                if object.contains_key("mask") {
                    return Err(RecognitionPackError::fatal_with_code(
                        RecognitionPackErrorCode::UnsupportedTarget,
                        format!(
                            "schema 0.6 target[{index}].mask is deprecated_in_vNext and must be migrated"
                        ),
                    ));
                }
                if let Some(method) = object.get("method").and_then(Value::as_str)
                    && method != "ncc"
                {
                    return Err(RecognitionPackError::fatal_with_code(
                        RecognitionPackErrorCode::UnsupportedTarget,
                        format!(
                            "schema 0.6 target[{index}].method='{method}' is deprecated_in_vNext and must be migrated"
                        ),
                    ));
                }
                &[
                    "type",
                    "id",
                    "template_path",
                    "region",
                    "threshold",
                    "method",
                    "rect_move",
                    "color_check",
                    "click",
                ][..]
            }
            "color" => &["type", "id", "region", "expected", "click"][..],
            "click_only" => &["type", "id", "click"][..],
            "ocr" => &[
                "type",
                "id",
                "region",
                "languages",
                "timeout_ms",
                "match_mode",
                "expected",
                "case_sensitive",
                "minimum_confidence",
                "model_ref",
                "model_sha256",
                "click",
            ][..],
            "nn" => &[
                "type",
                "id",
                "region",
                "model_ref",
                "model_sha256",
                "candidate_labels",
                "minimum_score",
                "selection",
                "expected_label",
                "timeout_ms",
                "click",
            ][..],
            other => {
                return Err(RecognitionPackError::fatal_with_code(
                    RecognitionPackErrorCode::UnsupportedTarget,
                    format!("schema 0.6 target[{index}] has unknown type '{other}'"),
                ));
            }
        };
        reject_unknown_fields(object, allowed, &format!("schema 0.6 target[{index}]"))?;
        for field in ["region", "rect_move", "click"] {
            if let Some(rect) = object.get(field)
                && !rect.is_null()
                && !(field == "region" && rect.is_string())
            {
                validate_strict_object(
                    rect,
                    &["x", "y", "width", "height"],
                    &format!("schema 0.6 target[{index}].{field}"),
                )?;
            }
        }
        if let Some(color_check) = object.get("color_check")
            && !color_check.is_null()
        {
            let color_check = validate_strict_object(
                color_check,
                &["region", "expected"],
                &format!("schema 0.6 target[{index}].color_check"),
            )?;
            if let Some(region) = color_check.get("region") {
                validate_strict_object(
                    region,
                    &["x", "y", "width", "height"],
                    &format!("schema 0.6 target[{index}].color_check.region"),
                )?;
            }
        }
    }
    Ok(())
}

fn validate_strict_object<'a>(
    value: &'a Value,
    allowed: &[&str],
    label: &str,
) -> RecognitionPackResult<&'a Map<String, Value>> {
    let object = value
        .as_object()
        .ok_or_else(|| RecognitionPackError::fatal(format!("{label} must be an object")))?;
    reject_unknown_fields(object, allowed, label)?;
    Ok(object)
}

fn reject_unknown_fields(
    object: &Map<String, Value>,
    allowed: &[&str],
    label: &str,
) -> RecognitionPackResult<()> {
    if let Some(field) = object
        .keys()
        .find(|field| !allowed.contains(&field.as_str()))
    {
        return Err(RecognitionPackError::fatal(format!(
            "{label} contains unknown field '{field}'"
        )));
    }
    Ok(())
}

fn validate_pack(
    asset_resolver: &dyn AssetResolver,
    pack: &RecognitionPack,
    errors: &mut Vec<String>,
) {
    if !matches!(
        pack.schema_version.as_str(),
        "0.1" | "0.3" | "0.4" | "0.5" | "0.6"
    ) {
        errors.push(format!(
            "unsupported schema_version '{}', expected one of '0.1', '0.3', '0.4', '0.5', '0.6'",
            pack.schema_version
        ));
    }
    match pack.coordinate_space {
        Some(space) if space.width > 0 && space.height > 0 => {}
        Some(space) => errors.push(format!(
            "coordinate_space dimensions must be positive: {}x{}",
            space.width, space.height
        )),
        None => errors.push(
            "coordinate_space is required; packs must declare their authored resolution"
                .to_string(),
        ),
    }
    validate_defaults(pack.defaults, errors);

    let mut seen = HashSet::new();
    for (index, target) in pack.targets.iter().enumerate() {
        let id = target.id();
        if id.is_empty() {
            errors.push(format!("target[{index}] id is empty"));
        } else if !seen.insert(id.to_string()) {
            errors.push(format!("target id '{id}' is duplicated"));
        }

        match target {
            RecognitionTarget::Template(target) => {
                if pack.schema_version == "0.6" {
                    if target.method != RecognitionMethod::Ncc {
                        errors.push(format!(
                            "target[{index}] method={:?} is deprecated_in_vNext; migrate to template+ncc, color, ocr, or nn",
                            target.method
                        ));
                    }
                    if target.mask.is_some() {
                        errors.push(format!(
                            "target[{index}] mask is deprecated_in_vNext and cannot be declared by schema 0.6"
                        ));
                    }
                }
                validate_region_shape(&target.region, &format!("target[{index}].region"), errors);
                if let Some(threshold) = target.threshold {
                    validate_template_threshold(
                        threshold,
                        &format!("target[{index}].threshold"),
                        errors,
                    );
                }
                if let Some(click) = target.click {
                    validate_rect_shape(click, &format!("target[{index}].click"), errors);
                }
                if let Some(rect_move) = target.rect_move {
                    validate_rect_shape(rect_move, &format!("target[{index}].rect_move"), errors);
                }
                if let Some(RecognitionMask::Bitmap { path }) = &target.mask {
                    validate_template_path(path, &format!("target[{index}].mask"), errors);
                }
                if let Some(check) = target.color_check {
                    validate_rect_shape(
                        check.region,
                        &format!("target[{index}].color_check.region"),
                        errors,
                    );
                }
                validate_template_path(&target.template_path, &format!("target[{index}]"), errors);
                if is_template_path_safe(&target.template_path)
                    && !asset_resolver.contains_asset(&target.template_path)
                {
                    errors.push(format!(
                        "target[{index}] template '{}' does not exist",
                        target.template_path
                    ));
                }
            }
            RecognitionTarget::Color(target) => {
                validate_rect_shape(target.region, &format!("target[{index}].region"), errors);
                if let Some(click) = target.click {
                    validate_rect_shape(click, &format!("target[{index}].click"), errors);
                }
            }
            RecognitionTarget::ClickOnly(target) => {
                validate_rect_shape(target.click, &format!("target[{index}].click"), errors);
            }
            RecognitionTarget::Ocr(target) => {
                if pack.schema_version != "0.6" {
                    errors.push(format!(
                        "target[{index}] type=ocr requires schema_version '0.6'"
                    ));
                }
                validate_region_shape(&target.region, &format!("target[{index}].region"), errors);
                validate_region_within_coordinate_space(
                    &target.region,
                    pack.coordinate_space,
                    &format!("target[{index}].region"),
                    errors,
                );
                validate_string_list(
                    &target.languages,
                    MAX_VISION_LANGUAGES,
                    "languages",
                    index,
                    errors,
                );
                validate_string_list(
                    &target.expected,
                    MAX_VISION_EXPECTED_VALUES,
                    "expected",
                    index,
                    errors,
                );
                validate_timeout(target.timeout_ms, index, errors);
                validate_unit_score(
                    target.minimum_confidence,
                    &format!("target[{index}].minimum_confidence"),
                    errors,
                );
                if target.model_ref != PPOCR_V6_MEDIUM_MODEL_REF {
                    errors.push(format!(
                        "target[{index}].model_ref must be '{PPOCR_V6_MEDIUM_MODEL_REF}' for OCR production targets"
                    ));
                }
                validate_model_reference(&target.model_ref, &target.model_sha256, index, errors);
                if let Some(click) = target.click {
                    validate_rect_shape(click, &format!("target[{index}].click"), errors);
                }
            }
            RecognitionTarget::Nn(target) => {
                if pack.schema_version != "0.6" {
                    errors.push(format!(
                        "target[{index}] type=nn requires schema_version '0.6'"
                    ));
                }
                validate_region_shape(&target.region, &format!("target[{index}].region"), errors);
                validate_region_within_coordinate_space(
                    &target.region,
                    pack.coordinate_space,
                    &format!("target[{index}].region"),
                    errors,
                );
                validate_string_list(
                    &target.candidate_labels,
                    MAX_VISION_LABELS,
                    "candidate_labels",
                    index,
                    errors,
                );
                validate_unit_score(
                    target.minimum_score,
                    &format!("target[{index}].minimum_score"),
                    errors,
                );
                validate_timeout(target.timeout_ms, index, errors);
                validate_model_reference(&target.model_ref, &target.model_sha256, index, errors);
                match target.selection {
                    NnSelectionMode::Best if target.expected_label.is_some() => {
                        errors.push(format!(
                            "target[{index}].expected_label must be omitted when selection='best'"
                        ))
                    }
                    NnSelectionMode::Label => match target.expected_label.as_deref() {
                        Some(expected)
                            if target
                                .candidate_labels
                                .iter()
                                .any(|label| label == expected) => {}
                        Some(expected) => errors.push(format!(
                            "target[{index}].expected_label '{expected}' is not in candidate_labels"
                        )),
                        None => errors.push(format!(
                            "target[{index}].expected_label is required when selection='label'"
                        )),
                    },
                    NnSelectionMode::Best => {}
                }
                if let Some(click) = target.click {
                    validate_rect_shape(click, &format!("target[{index}].click"), errors);
                }
            }
        }
    }
}

fn validate_defaults(defaults: RecognitionDefaults, errors: &mut Vec<String>) {
    validate_template_threshold(
        defaults.template_threshold,
        "defaults.template_threshold",
        errors,
    );
    if !defaults.color_max_distance.is_finite() || defaults.color_max_distance < 0.0 {
        errors.push(format!(
            "defaults.color_max_distance must be finite and >= 0.0: {}",
            defaults.color_max_distance
        ));
    }
}

fn validate_timeout(timeout_ms: u64, index: usize, errors: &mut Vec<String>) {
    if timeout_ms == 0 || timeout_ms > MAX_VISION_TIMEOUT_MS {
        errors.push(format!(
            "target[{index}].timeout_ms must be in 1..={MAX_VISION_TIMEOUT_MS}: {timeout_ms}"
        ));
    }
}

fn validate_unit_score(score: f32, label: &str, errors: &mut Vec<String>) {
    if !score.is_finite() || !(0.0..=1.0).contains(&score) {
        errors.push(format!("{label} must be finite and in 0.0..=1.0: {score}"));
    }
}

fn validate_string_list(
    values: &[String],
    maximum_count: usize,
    field: &str,
    index: usize,
    errors: &mut Vec<String>,
) {
    if values.is_empty() {
        errors.push(format!(
            "target[{index}].{field} must include at least one value"
        ));
        return;
    }
    if values.len() > maximum_count {
        errors.push(format!(
            "target[{index}].{field} contains {} values, limit is {maximum_count}",
            values.len()
        ));
    }
    let mut seen = HashSet::new();
    for (value_index, value) in values.iter().enumerate() {
        if value.trim().is_empty() {
            errors.push(format!(
                "target[{index}].{field}[{value_index}] must not be blank"
            ));
        }
        if value.len() > MAX_VISION_STRING_BYTES {
            errors.push(format!(
                "target[{index}].{field}[{value_index}] exceeds {MAX_VISION_STRING_BYTES} bytes"
            ));
        }
        if !seen.insert(value) {
            errors.push(format!(
                "target[{index}].{field} contains duplicate value '{value}'"
            ));
        }
    }
}

fn validate_model_reference(
    model_ref: &str,
    model_sha256: &str,
    index: usize,
    errors: &mut Vec<String>,
) {
    if model_ref.trim().is_empty() {
        errors.push(format!("target[{index}].model_ref must not be blank"));
    }
    if model_ref.len() > 255 {
        errors.push(format!("target[{index}].model_ref exceeds 255 bytes"));
    }
    if model_ref.contains(['/', '\\', ':']) || model_ref == "." || model_ref == ".." {
        errors.push(format!(
            "target[{index}].model_ref must be a logical identifier, not a host path"
        ));
    }
    if model_sha256.len() != 64
        || !model_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        errors.push(format!(
            "target[{index}].model_sha256 must be exactly 64 lowercase hexadecimal characters"
        ));
    }
}

fn validate_region_within_coordinate_space(
    region: &PackRegion,
    coordinate_space: Option<PackCoordinateSpace>,
    label: &str,
    errors: &mut Vec<String>,
) {
    let (PackRegion::Rect(rect), Some(space)) = (region, coordinate_space) else {
        return;
    };
    let right = i64::from(rect.x) + i64::from(rect.width);
    let bottom = i64::from(rect.y) + i64::from(rect.height);
    if right > i64::from(space.width) || bottom > i64::from(space.height) {
        errors.push(format!(
            "{label} exceeds coordinate_space {}x{}",
            space.width, space.height
        ));
    }
}

fn validate_template_threshold(threshold: f32, label: &str, errors: &mut Vec<String>) {
    if !threshold.is_finite() || !(0.0..=1.0).contains(&threshold) {
        errors.push(format!(
            "{label} must be finite and in 0.0..=1.0: {threshold}"
        ));
    }
}

fn validate_template_path(value: &str, label: &str, errors: &mut Vec<String>) {
    if value.is_empty() {
        errors.push(format!("{label} template_path is empty"));
    }
    if value.starts_with('/') {
        errors.push(format!("{label} template_path starts with '/'"));
    }
    if value.starts_with('\\') {
        errors.push(format!("{label} template_path starts with '\\'"));
    }
    if value.contains(':') {
        errors.push(format!("{label} template_path contains ':'"));
    }
    if value.contains('\\') {
        errors.push(format!("{label} template_path contains '\\'"));
    }
    if value
        .split('/')
        .any(|segment| segment == "." || segment == "..")
    {
        errors.push(format!(
            "{label} template_path contains '.' or '..' path segment"
        ));
    }
    if Path::new(value).is_absolute() {
        errors.push(format!("{label} template_path is absolute"));
    }
}

fn is_template_path_safe(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('/')
        && !value.starts_with('\\')
        && !value.contains(':')
        && !value.contains('\\')
        && !value
            .split('/')
            .any(|segment| segment == "." || segment == "..")
        && !Path::new(value).is_absolute()
}

fn validate_rect_shape(rect: PackRect, label: &str, errors: &mut Vec<String>) {
    if rect.x < 0 || rect.y < 0 {
        errors.push(format!(
            "{label} coordinates must be non-negative: ({}, {})",
            rect.x, rect.y
        ));
    }
    if rect.width <= 0 || rect.height <= 0 {
        errors.push(format!(
            "{label} dimensions must be positive: {}x{}",
            rect.width, rect.height
        ));
    }
}

fn validate_region_shape(region: &PackRegion, label: &str, errors: &mut Vec<String>) {
    match region {
        PackRegion::Rect(rect) => validate_rect_shape(*rect, label, errors),
        PackRegion::Keyword(value) if value == "full_frame" => {}
        PackRegion::Keyword(value) => errors.push(format!(
            "{label} string region must be 'full_frame', got '{value}'"
        )),
    }
}

fn target_region(
    target_id: &str,
    region: &PackRegion,
) -> RecognitionPackResult<Option<recognition::Rect>> {
    match region {
        PackRegion::Rect(rect) => Ok(Some((*rect).into())),
        PackRegion::Keyword(value) if value == "full_frame" => Ok(None),
        PackRegion::Keyword(value) => Err(RecognitionPackError::fatal(format!(
            "template target '{target_id}' has unsupported region '{value}'"
        ))),
    }
}

fn provider_frame(scene: &Scene) -> VisionProviderFrame<'_> {
    VisionProviderFrame {
        width: scene.width(),
        height: scene.height(),
        rgb8_pixels: scene.rgb8_pixels(),
    }
}

fn provider_region(
    scene: &Scene,
    target_id: &str,
    region: &PackRegion,
) -> RecognitionPackResult<PackRect> {
    let rect = match region {
        PackRegion::Rect(rect) => *rect,
        PackRegion::Keyword(value) if value == "full_frame" => PackRect {
            x: 0,
            y: 0,
            width: i32::try_from(scene.width()).map_err(|_| {
                RecognitionPackError::fatal(format!(
                    "vision target '{target_id}' frame width exceeds i32 range"
                ))
            })?,
            height: i32::try_from(scene.height()).map_err(|_| {
                RecognitionPackError::fatal(format!(
                    "vision target '{target_id}' frame height exceeds i32 range"
                ))
            })?,
        },
        PackRegion::Keyword(value) => {
            return Err(RecognitionPackError::fatal(format!(
                "vision target '{target_id}' has unsupported region '{value}'"
            )));
        }
    };
    if !rect_is_within(
        rect,
        PackRect {
            x: 0,
            y: 0,
            width: i32::try_from(scene.width())
                .map_err(|_| RecognitionPackError::fatal("scene width exceeds i32 range"))?,
            height: i32::try_from(scene.height())
                .map_err(|_| RecognitionPackError::fatal("scene height exceeds i32 range"))?,
        },
    ) {
        return Err(RecognitionPackError::fatal(format!(
            "vision target '{target_id}' region exceeds scene bounds"
        )));
    }
    Ok(rect)
}

fn provider_error(
    target_id: &str,
    capability: &str,
    err: VisionProviderError,
) -> RecognitionPackError {
    let code = if err.code() == VisionProviderErrorCode::InvalidResponse {
        RecognitionPackErrorCode::VisionProviderInvalidResponse
    } else {
        RecognitionPackErrorCode::VisionProviderFailure
    };
    RecognitionPackError::fatal_with_code(
        code,
        format!(
            "{capability} provider failed for target '{target_id}' with {:?}: {}",
            err.code(),
            err.message()
        ),
    )
}

fn ocr_execution_provider_binding_is_valid(execution: &OcrProviderExecutionEvidence) -> bool {
    matches!(
        (
            execution.requested_provider,
            execution.resolved_provider,
            execution.cpu_ep_registered,
            execution.cpu_fallback_disabled,
        ),
        (
            OcrExecutionProviderKind::Cpu,
            OcrExecutionProviderKind::Cpu,
            true,
            false,
        ) | (
            OcrExecutionProviderKind::Cuda,
            OcrExecutionProviderKind::Cuda,
            false,
            true,
        )
    )
}

#[derive(Debug)]
struct ValidatedOcrResult {
    text: String,
    confidence: Option<f32>,
    blocks: Vec<OcrTextEvidence>,
}

fn validate_ocr_result(
    result: OcrProviderResult,
    requested_region: PackRect,
) -> RecognitionPackResult<ValidatedOcrResult> {
    if result.text.len() > MAX_OCR_TEXT_BYTES {
        return Err(invalid_provider_response(format!(
            "OCR aggregate text exceeds {MAX_OCR_TEXT_BYTES} bytes"
        )));
    }
    validate_optional_provider_score(result.confidence, "OCR aggregate confidence")?;
    if result.blocks.len() > MAX_OCR_BLOCKS {
        return Err(invalid_provider_response(format!(
            "OCR returned {} blocks, limit is {MAX_OCR_BLOCKS}",
            result.blocks.len()
        )));
    }

    let mut blocks = Vec::with_capacity(result.blocks.len());
    for (index, block) in result.blocks.into_iter().enumerate() {
        if block.text.len() > MAX_VISION_STRING_BYTES {
            return Err(invalid_provider_response(format!(
                "OCR block[{index}] text exceeds {MAX_VISION_STRING_BYTES} bytes"
            )));
        }
        validate_optional_provider_score(
            block.confidence,
            &format!("OCR block[{index}] confidence"),
        )?;
        if !rect_is_within(block.rect, requested_region) {
            return Err(invalid_provider_response(format!(
                "OCR block[{index}] rect is outside the requested ROI"
            )));
        }
        blocks.push(OcrTextEvidence {
            text: block.text,
            rect: block.rect,
            confidence: block.confidence,
        });
    }
    blocks.sort_by(|left, right| {
        (
            left.rect.y,
            left.rect.x,
            left.rect.height,
            left.rect.width,
            &left.text,
        )
            .cmp(&(
                right.rect.y,
                right.rect.x,
                right.rect.height,
                right.rect.width,
                &right.text,
            ))
    });
    let text = if blocks.is_empty() {
        result.text
    } else {
        let text = blocks
            .iter()
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if text.len() > MAX_OCR_TEXT_BYTES {
            return Err(invalid_provider_response(format!(
                "sorted OCR block text exceeds {MAX_OCR_TEXT_BYTES} bytes"
            )));
        }
        text
    };
    Ok(ValidatedOcrResult {
        text,
        confidence: result.confidence,
        blocks,
    })
}

fn validate_nn_result(
    result: NnProviderResult,
    candidate_labels: &[String],
) -> RecognitionPackResult<Vec<NnLabelEvidence>> {
    if result.labels.len() > MAX_VISION_RESULTS {
        return Err(invalid_provider_response(format!(
            "NN returned {} labels, limit is {MAX_VISION_RESULTS}",
            result.labels.len()
        )));
    }
    let candidates: HashSet<&str> = candidate_labels.iter().map(String::as_str).collect();
    let mut seen = HashSet::new();
    let mut labels = Vec::with_capacity(result.labels.len());
    for (index, label) in result.labels.into_iter().enumerate() {
        if label.label.trim().is_empty() || label.label.len() > MAX_VISION_STRING_BYTES {
            return Err(invalid_provider_response(format!(
                "NN label[{index}] must be non-blank and at most {MAX_VISION_STRING_BYTES} bytes"
            )));
        }
        if !seen.insert(label.label.clone()) {
            return Err(invalid_provider_response(format!(
                "NN provider returned duplicate label '{}'",
                label.label
            )));
        }
        validate_provider_score(label.score, &format!("NN label[{index}] score"))?;
        labels.push(NnLabelEvidence {
            candidate: candidates.contains(label.label.as_str()),
            label: label.label,
            score: label.score,
        });
    }
    labels.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.label.cmp(&right.label))
    });
    Ok(labels)
}

fn validate_optional_provider_score(score: Option<f32>, label: &str) -> RecognitionPackResult<()> {
    if let Some(score) = score {
        validate_provider_score(score, label)?;
    }
    Ok(())
}

fn validate_provider_score(score: f32, label: &str) -> RecognitionPackResult<()> {
    if !score.is_finite() || !(0.0..=1.0).contains(&score) {
        return Err(invalid_provider_response(format!(
            "{label} must be finite and in 0.0..=1.0, got {score}"
        )));
    }
    Ok(())
}

fn invalid_provider_response(message: impl Into<String>) -> RecognitionPackError {
    RecognitionPackError::fatal_with_code(
        RecognitionPackErrorCode::VisionProviderInvalidResponse,
        message,
    )
}

fn rect_is_within(inner: PackRect, outer: PackRect) -> bool {
    if inner.x < outer.x
        || inner.y < outer.y
        || inner.width <= 0
        || inner.height <= 0
        || outer.width <= 0
        || outer.height <= 0
    {
        return false;
    }
    let inner_right = i64::from(inner.x) + i64::from(inner.width);
    let inner_bottom = i64::from(inner.y) + i64::from(inner.height);
    let outer_right = i64::from(outer.x) + i64::from(outer.width);
    let outer_bottom = i64::from(outer.y) + i64::from(outer.height);
    inner_right <= outer_right && inner_bottom <= outer_bottom
}

fn ocr_text_matches(text: &str, expected: &str, target: &OcrTarget) -> bool {
    if target.case_sensitive {
        match target.match_mode {
            OcrMatchMode::Exact => text == expected,
            OcrMatchMode::Contains => text.contains(expected),
        }
    } else {
        let text = text.to_lowercase();
        let expected = expected.to_lowercase();
        match target.match_mode {
            OcrMatchMode::Exact => text == expected,
            OcrMatchMode::Contains => text.contains(&expected),
        }
    }
}

fn primitive_error(target_id: &str, err: recognition::RecognitionError) -> RecognitionPackError {
    RecognitionPackError::fatal(format!(
        "recognition primitive failed for target '{target_id}': {err}"
    ))
}

fn unsupported_template_reason(target: &TemplateTarget) -> Option<String> {
    let mut reasons = Vec::new();
    if target.method != RecognitionMethod::Ncc {
        reasons.push(format!("method={:?}", target.method));
    }
    if target.mask.is_some() {
        reasons.push("mask".to_string());
    }
    if reasons.is_empty() {
        None
    } else {
        Some(reasons.join(", "))
    }
}

fn template_message(template_ok: bool, color_ok: bool) -> String {
    match (template_ok, color_ok) {
        (true, true) => "template passed".to_string(),
        (false, true) => "template score below threshold".to_string(),
        (true, false) => "color check failed".to_string(),
        (false, false) => "template score below threshold and color check failed".to_string(),
    }
}

fn default_template_threshold() -> f32 {
    0.90
}

fn default_color_max_distance() -> f32 {
    20.0
}

fn default_match_metric() -> RecognitionMatchMetric {
    RecognitionMatchMetric::CcorrNormed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEST_DIR_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn json_pack_parses() {
        let pack = load_pack_from_json_str(&template_pack_json("templates/button.png"))
            .expect("pack parsed");

        assert_eq!(pack.schema_version, "0.1");
        assert_eq!(pack.targets.len(), 1);
    }

    #[test]
    fn default_thresholds_are_usable() {
        let pack = load_pack_from_json_str(
            r#"{
                "schema_version": "0.1",
                "coordinate_space": {"width": 20, "height": 20},
                "targets": [
                    {"type": "click_only", "id": "tap", "click": {"x": 1, "y": 2, "width": 3, "height": 4}}
                ]
            }"#,
        )
        .expect("pack parsed");

        assert_eq!(pack.defaults.template_threshold, 0.90);
        assert_eq!(pack.defaults.color_max_distance, 20.0);
        assert_eq!(
            pack.defaults.match_metric,
            RecognitionMatchMetric::CcorrNormed
        );
        RecognitionEvaluator::new(TestDir::new().path.clone(), pack).expect("defaults valid");
    }

    #[test]
    fn schema_0_3_pack_is_supported() {
        let pack = load_pack_from_json_str(
            r#"{
                "schema_version": "0.3",
                "coordinate_space": {"width": 20, "height": 20},
                "defaults": {"match_metric": "ccoeff_normed"},
                "targets": [
                    {"type": "click_only", "id": "tap", "click": {"x": 1, "y": 2, "width": 3, "height": 4}}
                ]
            }"#,
        )
        .expect("pack parsed");

        let evaluator = RecognitionEvaluator::new(TestDir::new().path.clone(), pack)
            .expect("schema 0.3 accepted");
        assert_eq!(
            evaluator.default_match_metric(),
            MatchMetric::CorrelationCoefficientNormalized
        );
        assert_eq!(evaluator.unsupported_target_count(), 0);
        assert!(evaluator.unsupported_targets().is_empty());
    }

    #[test]
    fn schema_0_4_pack_round_trips_template_color_and_click_targets() {
        let fixture = TemplateFixture::new();
        let pack = load_pack_from_json_str(
            r#"{
                "schema_version": "0.4",
                "coordinate_space": {"width": 64, "height": 48},
                "defaults": {"match_metric": "ccoeff_normed"},
                "targets": [
                    {
                        "type": "template",
                        "id": "page/home",
                        "template_path": "templates/button.png",
                        "region": {"x": 12, "y": 10, "width": 28, "height": 24},
                        "threshold": 0.90
                    },
                    {
                        "type": "color",
                        "id": "color/ap",
                        "region": {"x": 0, "y": 0, "width": 6, "height": 6},
                        "expected": [30, 31, 32],
                        "click": {"x": 1, "y": 2, "width": 3, "height": 4}
                    },
                    {
                        "type": "click_only",
                        "id": "tap/settings",
                        "click": {"x": 5, "y": 6, "width": 7, "height": 8}
                    }
                ]
            }"#,
        )
        .expect("pack parsed");
        let evaluator =
            RecognitionEvaluator::new(fixture.dir.path.clone(), pack).expect("schema 0.4 accepted");

        assert_eq!(
            evaluator.default_match_metric(),
            MatchMetric::CorrelationCoefficientNormalized
        );
        assert!(evaluator.unsupported_targets().is_empty());
        assert!(
            evaluator
                .evaluate_target(&fixture.scene_with_template(), "page/home")
                .expect("template evaluation")
                .passed
        );
        assert!(
            evaluator
                .evaluate_target(&fixture.blank_scene(), "color/ap")
                .expect("color evaluation")
                .passed
        );
        assert_eq!(
            evaluator.get_click_target("tap/settings").expect("click"),
            rect(5, 6, 7, 8)
        );
    }

    #[test]
    fn schema_0_5_pack_loads_method_mask_and_fails_loud_when_used() {
        let fixture = TemplateFixture::new();
        let pack = load_pack_from_json_str(
            r#"{
                "schema_version": "0.5",
                "coordinate_space": {"width": 64, "height": 48},
                "targets": [
                    {
                        "type": "template",
                        "id": "template",
                        "template_path": "templates/button.png",
                        "region": {"x": 12, "y": 10, "width": 28, "height": 24},
                        "method": "rgb_count",
                        "mask": {"type": "range", "lower": 1, "upper": 255},
                        "rect_move": {"x": 0, "y": 10, "width": 5, "height": 2}
                    }
                ]
            }"#,
        )
        .expect("pack parsed");
        let evaluator =
            RecognitionEvaluator::new(fixture.dir.path.clone(), pack).expect("schema 0.5 accepted");

        assert_eq!(evaluator.unsupported_target_count(), 1);
        assert_eq!(evaluator.unsupported_targets()[0].id, "template");
        assert!(
            evaluator.unsupported_targets()[0]
                .reason
                .contains("method=RgbCount")
        );
        assert!(evaluator.unsupported_targets()[0].reason.contains("mask"));
        let err = evaluator
            .evaluate_target(&fixture.blank_scene(), "template")
            .expect_err("unsupported target fails loud");

        assert_fatal_contains(err, "unsupported recognition semantics");
    }

    #[test]
    fn schema_0_6_rejects_unknown_fields_without_changing_legacy_parsing() {
        let legacy = load_pack_from_json_str(
            r#"{
                "schema_version": "0.5",
                "coordinate_space": {"width": 2, "height": 1},
                "legacy_extension": true,
                "targets": [
                    {
                        "type": "click_only",
                        "id": "tap",
                        "click": {"x": 0, "y": 0, "width": 1, "height": 1},
                        "legacy_target_extension": true
                    }
                ]
            }"#,
        )
        .expect("legacy unknown fields remain accepted");
        assert_eq!(legacy.schema_version, "0.5");

        load_pack_from_json_str(
            r#"{
                "schema_version": "0.6",
                "converter_schema_version": "0.5",
                "generated": true,
                "generated_by": "actingcommand-resource-convert",
                "coordinate_space": {"width": 2, "height": 1},
                "targets": []
            }"#,
        )
        .expect("declared generator metadata remains valid in strict schema");

        let err = load_pack_from_json_str(
            r#"{
                "schema_version": "0.6",
                "generated": "true",
                "coordinate_space": {"width": 2, "height": 1},
                "targets": []
            }"#,
        )
        .expect_err("declared generator metadata retains its wire type");
        assert_fatal_contains(err, "generated' must be a boolean");

        let err = load_pack_from_json_str(
            r#"{
                "schema_version": "0.6",
                "coordinate_space": {"width": 2, "height": 1},
                "unexpected": true,
                "targets": []
            }"#,
        )
        .expect_err("v0.6 unknown root field rejected");
        assert_fatal_contains(err, "unknown field 'unexpected'");

        let err = load_pack_from_json_str(
            r#"{
                "schema_version": "0.6",
                "coordinate_space": {"width": 2, "height": 1},
                "targets": [
                    {
                        "type": "click_only",
                        "id": "tap",
                        "click": {"x": 0, "y": 0, "width": 1, "height": 1},
                        "unexpected": true
                    }
                ]
            }"#,
        )
        .expect_err("v0.6 unknown target field rejected");
        assert_fatal_contains(err, "unknown field 'unexpected'");
    }

    #[test]
    fn schema_0_6_rejects_deprecated_template_primitives() {
        for (field, value, expected) in [
            ("method", r#""rgb_count""#, "deprecated_in_vNext"),
            (
                "mask",
                r#"{"type":"range","lower":1,"upper":255}"#,
                "deprecated_in_vNext",
            ),
        ] {
            let json = format!(
                r#"{{
                    "schema_version": "0.6",
                    "coordinate_space": {{"width": 2, "height": 1}},
                    "targets": [{{
                        "type": "template",
                        "id": "legacy",
                        "template_path": "templates/legacy.png",
                        "region": "full_frame",
                        "{field}": {value}
                    }}]
                }}"#
            );
            let err = load_pack_from_json_str(&json).expect_err("deprecated primitive rejected");
            assert_eq!(err.code(), RecognitionPackErrorCode::UnsupportedTarget);
            assert_fatal_contains(err, expected);
        }
    }

    #[test]
    fn schema_0_6_ocr_exact_and_contains_are_runtime_owned() {
        let scene = Scene::from_rgb8(2, 1, &[1, 2, 3, 4, 5, 6]).expect("scene");
        let provider = Arc::new(TestVisionProvider {
            execution: None,
            ocr: Ok(OcrProviderResult {
                text: "provider aggregate is not authoritative".to_string(),
                blocks: vec![OcrProviderTextBlock {
                    text: "Hello Runtime".to_string(),
                    rect: rect(0, 0, 2, 1),
                    confidence: Some(0.95),
                }],
                confidence: Some(0.95),
            }),
            nn: Err(VisionProviderError::new(
                VisionProviderErrorCode::Unavailable,
                "unused",
            )),
        });
        let exact = vision_evaluator(
            ocr_pack_json("exact", "hello runtime", 0.90),
            provider.clone(),
        );
        let contains = vision_evaluator(ocr_pack_json("contains", "runtime", 0.90), provider);

        let exact_result = exact.evaluate_target(&scene, "ocr/page").expect("exact");
        assert!(exact_result.passed);
        assert_eq!(exact_result.kind, TargetKind::Ocr);
        assert_eq!(
            exact_result.ocr.expect("ocr evidence").text,
            "Hello Runtime"
        );
        assert!(
            contains
                .evaluate_target(&scene, "ocr/page")
                .expect("contains")
                .passed
        );
    }

    #[test]
    fn post_admission_ocr_requires_execution_evidence_without_changing_normal_evaluation() {
        let provider = Arc::new(TestVisionProvider {
            execution: None,
            ocr: Ok(OcrProviderResult {
                text: "hello".to_string(),
                blocks: vec![OcrProviderTextBlock {
                    text: "hello".to_string(),
                    rect: rect(0, 0, 2, 1),
                    confidence: Some(1.0),
                }],
                confidence: Some(1.0),
            }),
            nn: Err(VisionProviderError::new(
                VisionProviderErrorCode::Unavailable,
                "unused",
            )),
        });
        let evaluator = vision_evaluator(ocr_pack_json("exact", "hello", 0.0), provider);
        let scene = Scene::from_rgb8(2, 1, &[0; 6]).expect("scene");

        assert!(
            evaluator
                .evaluate_target(&scene, "ocr/page")
                .expect("normal OCR compatibility path")
                .passed
        );
        let error = evaluator
            .evaluate_ocr_observation(&scene, "ocr/page")
            .expect_err("attestation-free observation must fail closed");
        assert_eq!(
            error.code(),
            RecognitionPackErrorCode::VisionProviderInvalidResponse
        );
        assert!(error.message().contains("missing execution evidence"));
    }

    #[test]
    fn post_admission_ocr_requires_provider_specific_execution_evidence() {
        let scene = Scene::from_rgb8(2, 1, &[0; 6]).expect("scene");
        for provider in [
            OcrExecutionProviderKind::Cpu,
            OcrExecutionProviderKind::Cuda,
        ] {
            let evaluator = observation_evaluator(execution_evidence(provider));
            let evaluation = evaluator
                .evaluate_ocr_observation(&scene, "ocr/page")
                .expect("valid provider-specific execution evidence");
            assert_eq!(evaluation.text, "hello");
            assert_eq!(evaluation.execution.requested_provider, provider);
            assert_eq!(evaluation.execution.resolved_provider, provider);
        }

        let valid_cpu = execution_evidence(OcrExecutionProviderKind::Cpu);
        let valid_cuda = execution_evidence(OcrExecutionProviderKind::Cuda);
        let mut invalid = Vec::new();

        let mut cpu_fallback_disabled = valid_cpu.clone();
        cpu_fallback_disabled.cpu_fallback_disabled = true;
        invalid.push(("CPU fallback-disable inversion", cpu_fallback_disabled));

        let mut cpu_ep_missing = valid_cpu.clone();
        cpu_ep_missing.cpu_ep_registered = false;
        invalid.push(("CPU EP missing", cpu_ep_missing));

        let mut cuda_cpu_ep_registered = valid_cuda.clone();
        cuda_cpu_ep_registered.cpu_ep_registered = true;
        invalid.push(("CUDA registered CPU EP", cuda_cpu_ep_registered));

        let mut cuda_fallback_enabled = valid_cuda.clone();
        cuda_fallback_enabled.cpu_fallback_disabled = false;
        invalid.push(("CUDA CPU fallback enabled", cuda_fallback_enabled));

        let mut provider_mismatch = valid_cpu.clone();
        provider_mismatch.resolved_provider = OcrExecutionProviderKind::Cuda;
        invalid.push(("requested/resolved mismatch", provider_mismatch));

        let mut model_mismatch = valid_cpu.clone();
        model_mismatch.model_sha256 = "b".repeat(64);
        invalid.push(("model mismatch", model_mismatch));

        let mut incomplete = valid_cpu.clone();
        incomplete.complete = false;
        invalid.push(("incomplete evidence", incomplete));

        let mut fallback_allowed = valid_cpu.clone();
        fallback_allowed.fallback_forbidden = false;
        invalid.push(("fallback allowed", fallback_allowed));

        let mut fallback_observed = valid_cpu;
        fallback_observed.fallback_observed = Some(true);
        invalid.push(("fallback observed", fallback_observed));

        for (label, execution) in invalid {
            let error = observation_evaluator(execution)
                .evaluate_ocr_observation(&scene, "ocr/page")
                .expect_err(label);
            assert_eq!(
                error.code(),
                RecognitionPackErrorCode::VisionProviderInvalidResponse,
                "{label}"
            );
            assert!(
                error
                    .message()
                    .contains("execution evidence does not match target"),
                "{label}: {}",
                error.message()
            );
        }
    }

    #[test]
    fn mixed_vision_pack_constructs_and_non_vision_targets_evaluate_without_provider() {
        let fixture = TemplateFixture::new();
        let evaluator =
            RecognitionEvaluator::new(fixture.dir.path.clone(), fixture.mixed_vision_pack())
                .expect("mixed evaluator does not require an unselected vision provider");
        let scene = fixture.scene_with_template();

        assert!(
            evaluator
                .evaluate_target(&scene, "template")
                .expect("template evaluation")
                .passed
        );
        assert!(
            evaluator
                .evaluate_target(&scene, "color")
                .expect("color evaluation")
                .passed
        );

        for target_id in ["ocr/page", "nn/page"] {
            let error = evaluator
                .evaluate_target(&scene, target_id)
                .expect_err("selected vision target requires a provider");
            assert_eq!(
                error.code(),
                RecognitionPackErrorCode::VisionProviderMissing
            );
            assert!(error.message().contains(target_id), "{}", error.message());
        }
    }

    #[test]
    fn vision_model_requirements_are_selected_target_scoped() {
        let fixture = TemplateFixture::new();
        let provider = Arc::new(TrackingVisionProvider::new(false));
        let evaluator = RecognitionEvaluator::with_vision_provider(
            fixture.mixed_vision_pack(),
            Arc::new(FsAssetResolver::new(fixture.dir.path.clone())),
            provider.clone(),
        )
        .expect("mixed evaluator");
        let scene = fixture.scene_with_template();

        assert_eq!(provider.counts(), (0, 0, 0, 0));
        evaluator
            .evaluate_target(&scene, "template")
            .expect("template evaluation");
        evaluator
            .evaluate_target(&scene, "color")
            .expect("color evaluation");
        assert_eq!(provider.counts(), (0, 0, 0, 0));

        evaluator
            .evaluate_target(&scene, "ocr/page")
            .expect("ocr evaluation");
        assert_eq!(provider.counts(), (1, 0, 1, 0));

        evaluator
            .evaluate_target(&scene, "nn/page")
            .expect("nn evaluation");
        assert_eq!(provider.counts(), (1, 1, 1, 1));
    }

    #[test]
    fn vision_model_mismatch_prevents_inference() {
        let fixture = TemplateFixture::new();
        let provider = Arc::new(TrackingVisionProvider::new(true));
        let evaluator = RecognitionEvaluator::with_vision_provider(
            fixture.mixed_vision_pack(),
            Arc::new(FsAssetResolver::new(fixture.dir.path.clone())),
            provider.clone(),
        )
        .expect("mixed evaluator");
        let scene = fixture.scene_with_template();

        let ocr_error = evaluator
            .evaluate_ocr_observation(&scene, "ocr/page")
            .expect_err("OCR model mismatch is fail-closed");
        assert_eq!(
            ocr_error.code(),
            RecognitionPackErrorCode::VisionProviderFailure
        );
        assert!(ocr_error.message().contains("ocr admission"));
        assert_eq!(provider.counts(), (1, 0, 0, 0));

        let nn_error = evaluator
            .evaluate_target(&scene, "nn/page")
            .expect_err("NN model mismatch is fail-closed");
        assert_eq!(
            nn_error.code(),
            RecognitionPackErrorCode::VisionProviderFailure
        );
        assert!(nn_error.message().contains("nn admission"));
        assert_eq!(provider.counts(), (1, 1, 0, 0));
    }

    #[test]
    fn ocr_invalid_confidence_and_out_of_roi_blocks_fail_closed() {
        for result in [
            OcrProviderResult {
                text: "hello".to_string(),
                blocks: Vec::new(),
                confidence: Some(f32::NAN),
            },
            OcrProviderResult {
                text: "hello".to_string(),
                blocks: vec![OcrProviderTextBlock {
                    text: "hello".to_string(),
                    rect: rect(1, 0, 2, 1),
                    confidence: Some(1.0),
                }],
                confidence: Some(1.0),
            },
        ] {
            let evaluator = vision_evaluator(
                ocr_pack_json("exact", "hello", 0.0),
                Arc::new(TestVisionProvider {
                    execution: None,
                    ocr: Ok(result),
                    nn: Err(VisionProviderError::new(
                        VisionProviderErrorCode::Unavailable,
                        "unused",
                    )),
                }),
            );
            let scene = Scene::from_rgb8(2, 1, &[0; 6]).expect("scene");
            let err = evaluator
                .evaluate_target(&scene, "ocr/page")
                .expect_err("invalid provider output rejected");
            assert_eq!(
                err.code(),
                RecognitionPackErrorCode::VisionProviderInvalidResponse
            );
        }
    }

    #[test]
    fn nn_label_selection_ignores_unknown_labels_and_sorts_deterministically() {
        let provider = Arc::new(TestVisionProvider {
            execution: None,
            ocr: Err(VisionProviderError::new(
                VisionProviderErrorCode::Unavailable,
                "unused",
            )),
            nn: Ok(NnProviderResult {
                labels: vec![
                    NnProviderLabel {
                        label: "home".to_string(),
                        score: 0.80,
                    },
                    NnProviderLabel {
                        label: "provider.private".to_string(),
                        score: 0.99,
                    },
                    NnProviderLabel {
                        label: "settings".to_string(),
                        score: 0.70,
                    },
                ],
            }),
        });
        let evaluator = vision_evaluator(nn_pack_json(), provider);
        let scene = Scene::from_rgb8(2, 1, &[0; 6]).expect("scene");

        let evaluation = evaluator.evaluate_target(&scene, "nn/page").expect("nn");

        assert!(evaluation.passed);
        let nn = evaluation.nn.expect("nn evidence");
        assert_eq!(nn.selected_label.as_deref(), Some("home"));
        assert_eq!(nn.selected_score, Some(0.80));
        assert_eq!(nn.labels[0].label, "provider.private");
        assert!(!nn.labels[0].candidate);
    }

    #[test]
    fn template_target_hit_passes() {
        let fixture = TemplateFixture::new();
        let evaluator = fixture.template_evaluator(0.90);
        let scene = fixture.scene_with_template();

        let evaluation = evaluator
            .evaluate_target(&scene, "template")
            .expect("evaluation");

        assert!(evaluation.passed);
        assert_eq!(evaluation.kind, TargetKind::Template);
        let template = evaluation.template.expect("template result");
        assert!(template.score >= 0.99, "score was {}", template.score);
    }

    #[test]
    fn template_target_below_threshold_fails() {
        let fixture = TemplateFixture::new();
        let evaluator = fixture.template_evaluator(0.99);
        let scene = fixture.blank_scene();

        let evaluation = evaluator
            .evaluate_target(&scene, "template")
            .expect("evaluation");

        assert!(!evaluation.passed);
        let template = evaluation.template.expect("template result");
        assert!(template.score < template.threshold);
    }

    #[test]
    fn ccoeff_match_metric_evaluates_template_targets() {
        let fixture = TemplateFixture::new();
        let evaluator = fixture.template_evaluator_with_defaults(
            RecognitionDefaults {
                match_metric: RecognitionMatchMetric::CcoeffNormed,
                ..RecognitionDefaults::default()
            },
            None,
        );
        let scene = fixture.scene_with_template();

        let evaluation = evaluator
            .evaluate_target(&scene, "template")
            .expect("evaluation");

        assert!(evaluation.passed);
        assert_eq!(
            evaluator.default_match_metric(),
            MatchMetric::CorrelationCoefficientNormalized
        );
        let template = evaluation.template.expect("template result");
        assert!(
            template.raw_score >= 0.99,
            "score was {}",
            template.raw_score
        );
    }

    #[test]
    fn full_frame_region_evaluates_template_targets() {
        let fixture = TemplateFixture::new();
        let evaluator =
            fixture.template_evaluator_with_region(PackRegion::Keyword("full_frame".to_string()));
        let scene = fixture.scene_with_template();

        let evaluation = evaluator
            .evaluate_target(&scene, "template")
            .expect("evaluation");

        assert!(evaluation.passed);
    }

    #[test]
    fn target_threshold_overrides_default_threshold() {
        let fixture = TemplateFixture::new();
        let evaluator = fixture.template_evaluator_with_defaults(
            RecognitionDefaults {
                template_threshold: 1.0,
                ..RecognitionDefaults::default()
            },
            Some(0.90),
        );
        let scene = fixture.scene_with_template();

        let evaluation = evaluator
            .evaluate_target(&scene, "template")
            .expect("evaluation");

        let template = evaluation.template.expect("template result");
        assert_eq!(template.threshold, 0.90);
        assert!(evaluation.passed);
    }

    #[test]
    fn template_evaluation_returns_raw_and_normalized_scores() {
        let fixture = TemplateFixture::new();
        let evaluator = fixture.template_evaluator(0.90);
        let scene = fixture.scene_with_template();

        let evaluation = evaluator
            .evaluate_target(&scene, "template")
            .expect("evaluation");
        let template = evaluation.template.expect("template result");

        assert!(template.raw_score >= 0.99);
        assert!((0.0..=1.0).contains(&template.score));
        assert_eq!((template.width, template.height), (8, 6));
    }

    #[test]
    fn template_region_evaluation_returns_ordered_rows_and_selected_winner() {
        let fixture = TemplateFixture::new();
        let evaluator = fixture.template_evaluator_with_defaults(
            RecognitionDefaults {
                template_threshold: 0.90,
                match_metric: RecognitionMatchMetric::CcoeffNormed,
                ..RecognitionDefaults::default()
            },
            None,
        );
        let scene = fixture.scene_with_template();
        let regions = [rect(0, 0, 16, 16), rect(12, 10, 28, 24)];

        let batch = evaluator
            .evaluate_template_regions(&scene, "template", &regions)
            .expect("region evaluation");

        assert_eq!(batch.target_id, "template");
        assert_eq!(batch.rows.len(), 2);
        let first = &batch.rows[0];
        assert_eq!(first.index, 0);
        assert_eq!(first.requested_region, regions[0]);
        assert_eq!(first.metric, RecognitionMatchMetric::CcoeffNormed);
        assert!(first.raw_score.is_finite());
        assert!((0.0..=1.0).contains(&first.normalized_score));
        assert_eq!(first.threshold, 0.90);
        assert!(!first.passed);
        assert!(!first.selected);

        let winner = &batch.rows[1];
        assert_eq!(winner.index, 1);
        assert_eq!(winner.requested_region, regions[1]);
        assert_eq!(winner.metric, RecognitionMatchMetric::CcoeffNormed);
        assert_eq!(winner.matched_rect, rect(20, 15, 8, 6));
        assert!(winner.raw_score >= 0.99);
        assert!(winner.normalized_score >= 0.99);
        assert_eq!(winner.threshold, 0.90);
        assert!(winner.passed);
        assert!(winner.selected);

        let stored = serde_json::to_value(&batch).expect("serialized region evaluation");
        assert_eq!(stored["target_id"], "template");
        assert_eq!(stored["rows"][1]["index"], 1);
        assert_eq!(stored["rows"][1]["metric"], "ccoeff_normed");
        assert_eq!(stored["rows"][1]["requested_region"]["x"], 12);
        assert_eq!(stored["rows"][1]["matched_rect"]["x"], 20);
        assert_eq!(stored["rows"][1]["selected"], true);
    }

    #[test]
    fn template_region_evaluation_uses_lowest_index_tie_and_selects_none_when_all_fail() {
        let fixture = TemplateFixture::new();
        let evaluator = fixture.template_evaluator(0.90);
        let scene = fixture.scene_with_template();
        let tied_regions = [rect(12, 10, 28, 24), rect(18, 13, 16, 12)];

        let tied = evaluator
            .evaluate_template_regions(&scene, "template", &tied_regions)
            .expect("tied region evaluation");

        assert!(tied.rows.iter().all(|row| row.passed));
        assert_eq!(tied.rows[0].normalized_score, tied.rows[1].normalized_score);
        assert!(tied.rows[0].selected);
        assert!(!tied.rows[1].selected);

        let none_evaluator = fixture.template_evaluator(1.0);
        let failed = none_evaluator
            .evaluate_template_regions(
                &fixture.blank_scene(),
                "template",
                &[rect(0, 0, 16, 16), rect(16, 0, 16, 16)],
            )
            .expect("below-threshold region evaluation");
        assert!(failed.rows.iter().all(|row| !row.passed));
        assert!(failed.rows.iter().all(|row| !row.selected));
    }

    #[test]
    fn template_region_evaluation_rejects_invalid_batch_without_partial_result() {
        let fixture = TemplateFixture::new();
        let evaluator = fixture.template_evaluator(0.90);
        let scene = fixture.scene_with_template();

        assert_fatal_contains(
            evaluator
                .evaluate_template_regions(&scene, "template", &[])
                .expect_err("empty batch rejected"),
            "1..=64",
        );
        assert_fatal_contains(
            evaluator
                .evaluate_template_regions(&scene, "template", &vec![rect(0, 0, 8, 6); 65])
                .expect_err("oversized batch rejected"),
            "1..=64",
        );
        assert_fatal_contains(
            evaluator
                .evaluate_template_regions(
                    &scene,
                    "template",
                    &[rect(12, 10, 28, 24), rect(12, 10, 28, 24)],
                )
                .expect_err("duplicate region rejected"),
            "duplicates region[0]",
        );
        assert_fatal_contains(
            evaluator
                .evaluate_template_regions(
                    &scene,
                    "template",
                    &[rect(12, 10, 28, 24), rect(60, 40, 8, 8)],
                )
                .expect_err("out-of-bounds region rejected"),
            "region[1] must be nonempty and within scene bounds",
        );

        let non_template = RecognitionEvaluator::new(fixture.dir.path.clone(), click_pack())
            .expect("non-template evaluator");
        assert_fatal_contains(
            non_template
                .evaluate_template_regions(&red_scene(), "tap", &[rect(0, 0, 8, 8)])
                .expect_err("non-template target rejected"),
            "is not a template target",
        );
    }

    #[test]
    fn color_target_red_expected_red_passes() {
        let dir = TestDir::new();
        let evaluator = RecognitionEvaluator::new(dir.path.clone(), color_pack([255, 0, 0]))
            .expect("evaluator");
        let scene = red_scene();

        let evaluation = evaluator.evaluate_target(&scene, "color").expect("color");

        assert!(evaluation.passed);
        assert_eq!(evaluation.kind, TargetKind::Color);
        assert_eq!(evaluation.color.expect("color result").mean, [255, 0, 0]);
    }

    #[test]
    fn color_target_red_expected_blue_fails() {
        let dir = TestDir::new();
        let evaluator = RecognitionEvaluator::new(dir.path.clone(), color_pack([0, 0, 255]))
            .expect("evaluator");
        let scene = red_scene();

        let evaluation = evaluator.evaluate_target(&scene, "color").expect("color");

        assert!(!evaluation.passed);
        assert!(evaluation.color.expect("color result").distance > 300.0);
    }

    #[test]
    fn click_only_target_loads() {
        let dir = TestDir::new();
        let evaluator =
            RecognitionEvaluator::new(dir.path.clone(), click_pack()).expect("evaluator");

        assert_eq!(
            evaluator.get_click_target("tap").expect("click"),
            PackRect {
                x: 10,
                y: 20,
                width: 30,
                height: 40
            }
        );
    }

    #[test]
    fn click_only_target_cannot_be_evaluated() {
        let dir = TestDir::new();
        let evaluator =
            RecognitionEvaluator::new(dir.path.clone(), click_pack()).expect("evaluator");
        let err = evaluator
            .evaluate_target(&red_scene(), "tap")
            .expect_err("click-only evaluation rejected");

        assert_fatal_contains(err, "click-only target");
    }

    #[test]
    fn missing_template_file_is_fatal_in_new() {
        let dir = TestDir::new();
        let pack =
            load_pack_from_json_str(&template_pack_json("templates/missing.png")).expect("pack");
        let err = RecognitionEvaluator::new(dir.path.clone(), pack).expect_err("missing template");

        assert_fatal_contains(err, "does not exist");
    }

    #[test]
    fn broken_template_png_is_fatal_in_evaluate() {
        let dir = TestDir::new();
        dir.write("templates/broken.png", b"not png")
            .expect("write broken");
        let pack =
            load_pack_from_json_str(&template_pack_json("templates/broken.png")).expect("pack");
        let evaluator = RecognitionEvaluator::new(dir.path.clone(), pack).expect("evaluator");

        let err = evaluator
            .evaluate_target(&red_scene(), "template")
            .expect_err("broken PNG");

        assert_fatal_contains(err, "recognition primitive failed");
    }

    #[test]
    fn empty_id_is_fatal() {
        let dir = TestDir::new();
        let pack = click_pack_with_id("");
        let err = RecognitionEvaluator::new(dir.path.clone(), pack).expect_err("empty id");

        assert_fatal_contains(err, "id is empty");
    }

    #[test]
    fn duplicate_id_is_fatal() {
        let dir = TestDir::new();
        let pack = RecognitionPack {
            targets: vec![click_target("same"), click_target("same")],
            ..base_pack()
        };
        let err = RecognitionEvaluator::new(dir.path.clone(), pack).expect_err("duplicate id");

        assert_fatal_contains(err, "duplicated");
    }

    #[test]
    fn empty_template_path_is_fatal() {
        let dir = TestDir::new();
        let pack = load_pack_from_json_str(&template_pack_json("")).expect("pack");
        let err = RecognitionEvaluator::new(dir.path.clone(), pack).expect_err("empty path");

        assert_fatal_contains(err, "template_path is empty");
    }

    #[test]
    fn absolute_template_path_is_fatal() {
        let dir = TestDir::new();
        let pack =
            load_pack_from_json_str(&template_pack_json("C:/tmp/template.png")).expect("pack");
        let err = RecognitionEvaluator::new(dir.path.clone(), pack).expect_err("absolute path");

        assert_fatal_contains(err, "contains ':'");
    }

    #[test]
    fn unsafe_template_path_segments_are_fatal() {
        for path in [
            "templates/../button.png",
            "templates/./button.png",
            "templates\\button.png",
            "templates:button.png",
        ] {
            let dir = TestDir::new();
            let pack = load_pack_from_json_str(&template_pack_json(path)).expect("pack");
            let err = RecognitionEvaluator::new(dir.path.clone(), pack).expect_err("unsafe path");
            assert_eq!(err.severity(), RecognitionPackErrorSeverity::Fatal);
        }
    }

    #[test]
    fn template_threshold_out_of_range_is_fatal() {
        for threshold in [-0.1, 1.1] {
            let dir = TestDir::new();
            let pack = RecognitionPack {
                defaults: RecognitionDefaults {
                    template_threshold: threshold,
                    ..RecognitionDefaults::default()
                },
                ..click_pack()
            };
            let err = RecognitionEvaluator::new(dir.path.clone(), pack).expect_err("threshold");
            assert_fatal_contains(err, "template_threshold");
        }
    }

    #[test]
    fn color_max_distance_negative_is_fatal() {
        let dir = TestDir::new();
        let pack = RecognitionPack {
            defaults: RecognitionDefaults {
                color_max_distance: -0.1,
                ..RecognitionDefaults::default()
            },
            ..click_pack()
        };
        let err = RecognitionEvaluator::new(dir.path.clone(), pack).expect_err("distance");

        assert_fatal_contains(err, "color_max_distance");
    }

    #[test]
    fn nan_and_infinite_thresholds_are_fatal() {
        let invalid_defaults = [
            RecognitionDefaults {
                template_threshold: f32::NAN,
                ..RecognitionDefaults::default()
            },
            RecognitionDefaults {
                template_threshold: f32::INFINITY,
                ..RecognitionDefaults::default()
            },
            RecognitionDefaults {
                color_max_distance: f32::NAN,
                ..RecognitionDefaults::default()
            },
            RecognitionDefaults {
                color_max_distance: f32::INFINITY,
                ..RecognitionDefaults::default()
            },
        ];

        for defaults in invalid_defaults {
            let dir = TestDir::new();
            let pack = RecognitionPack {
                defaults,
                ..click_pack()
            };
            let err = RecognitionEvaluator::new(dir.path.clone(), pack).expect_err("invalid");
            assert_eq!(err.severity(), RecognitionPackErrorSeverity::Fatal);
        }
    }

    #[test]
    fn template_with_passing_color_check_passes() {
        let fixture = TemplateFixture::new();
        let evaluator = fixture.template_with_color_evaluator([30, 31, 32]);
        let scene = fixture.scene_with_template();

        let evaluation = evaluator
            .evaluate_target(&scene, "template")
            .expect("evaluation");

        assert!(evaluation.passed);
        assert!(evaluation.color.is_some());
    }

    #[test]
    fn template_with_failing_color_check_fails() {
        let fixture = TemplateFixture::new();
        let evaluator = fixture.template_with_color_evaluator([255, 0, 0]);
        let scene = fixture.scene_with_template();

        let evaluation = evaluator
            .evaluate_target(&scene, "template")
            .expect("evaluation");

        assert!(!evaluation.passed);
        assert!(evaluation.template.expect("template").score >= 0.99);
        assert!(evaluation.color.expect("color").distance > 20.0);
    }

    #[test]
    fn coordinate_space_mismatch_is_fatal() {
        let dir = TestDir::new();
        let pack = RecognitionPack {
            coordinate_space: Some(PackCoordinateSpace {
                width: 11,
                height: 22,
            }),
            targets: vec![click_target("tap")],
            ..base_pack()
        };
        let evaluator = RecognitionEvaluator::new(dir.path.clone(), pack).expect("evaluator");
        let err = evaluator
            .evaluate_target(&red_scene(), "tap")
            .expect_err("coordinate mismatch");

        assert_fatal_contains(err, "coordinate_space");
    }

    #[test]
    fn missing_coordinate_space_is_fatal_in_new() {
        let dir = TestDir::new();
        let pack = RecognitionPack {
            coordinate_space: None,
            targets: vec![click_target("tap")],
            ..base_pack()
        };
        let err = RecognitionEvaluator::new(dir.path.clone(), pack)
            .expect_err("missing coordinate_space");

        assert_fatal_contains(err, "coordinate_space is required");
    }

    #[test]
    fn get_click_target_handles_all_target_kinds() {
        let fixture = TemplateFixture::new();
        let pack = RecognitionPack {
            targets: vec![
                RecognitionTarget::ClickOnly(ClickOnlyTarget {
                    id: "tap".to_string(),
                    click: rect(1, 2, 3, 4),
                }),
                RecognitionTarget::Template(TemplateTarget {
                    id: "template".to_string(),
                    template_path: "templates/button.png".to_string(),
                    region: PackRegion::Rect(rect(12, 10, 28, 24)),
                    threshold: None,
                    method: RecognitionMethod::Ncc,
                    mask: None,
                    rect_move: None,
                    color_check: None,
                    click: Some(rect(5, 6, 7, 8)),
                }),
                RecognitionTarget::Color(ColorTarget {
                    id: "color".to_string(),
                    region: rect(0, 0, 10, 10),
                    expected: [255, 0, 0],
                    click: Some(rect(9, 10, 11, 12)),
                }),
                RecognitionTarget::Color(ColorTarget {
                    id: "no-click".to_string(),
                    region: rect(0, 0, 10, 10),
                    expected: [255, 0, 0],
                    click: None,
                }),
            ],
            ..base_pack()
        };
        let evaluator = RecognitionEvaluator::new(fixture.dir.path.clone(), pack).expect("eval");

        assert_eq!(
            evaluator.get_click_target("tap").expect("tap"),
            rect(1, 2, 3, 4)
        );
        assert_eq!(
            evaluator.get_click_target("template").expect("template"),
            rect(5, 6, 7, 8)
        );
        assert_eq!(
            evaluator.get_click_target("color").expect("color"),
            rect(9, 10, 11, 12)
        );
        assert_fatal_contains(
            evaluator
                .get_click_target("no-click")
                .expect_err("missing click"),
            "has no click",
        );
        assert_fatal_contains(
            evaluator
                .get_click_target("missing")
                .expect_err("missing id"),
            "not found",
        );
    }

    #[test]
    fn new_collects_multiple_errors() {
        let dir = TestDir::new();
        let pack = RecognitionPack {
            defaults: RecognitionDefaults {
                template_threshold: 1.5,
                color_max_distance: -1.0,
                ..RecognitionDefaults::default()
            },
            targets: vec![
                RecognitionTarget::Template(TemplateTarget {
                    id: "".to_string(),
                    template_path: "".to_string(),
                    region: PackRegion::Rect(rect(-1, 0, 0, 4)),
                    threshold: None,
                    method: RecognitionMethod::Ncc,
                    mask: None,
                    rect_move: None,
                    color_check: None,
                    click: None,
                }),
                RecognitionTarget::Color(ColorTarget {
                    id: "".to_string(),
                    region: rect(0, -1, 4, 0),
                    expected: [0, 0, 0],
                    click: None,
                }),
            ],
            ..base_pack()
        };
        let err = RecognitionEvaluator::new(dir.path.clone(), pack).expect_err("many errors");
        let message = err.message();

        assert!(message.contains("template_threshold"));
        assert!(message.contains("color_max_distance"));
        assert!(message.contains("target[0] id is empty"));
        assert!(message.contains("target[1] id is empty"));
        assert!(message.contains("template_path is empty"));
        assert!(message.contains("dimensions must be positive"));
    }

    fn template_pack_json(path: &str) -> String {
        format!(
            r#"{{
                "schema_version": "0.1",
                "coordinate_space": {{"width": 20, "height": 20}},
                "defaults": {{"template_threshold": 0.90, "color_max_distance": 20.0}},
                "targets": [
                    {{
                        "type": "template",
                        "id": "template",
                        "template_path": "{path}",
                        "region": {{"x": 12, "y": 10, "width": 28, "height": 24}}
                    }}
                ]
            }}"#
        )
    }

    fn base_pack() -> RecognitionPack {
        RecognitionPack {
            schema_version: "0.1".to_string(),
            game: None,
            server: None,
            locale: None,
            coordinate_space: Some(PackCoordinateSpace {
                width: 20,
                height: 20,
            }),
            defaults: RecognitionDefaults::default(),
            targets: Vec::new(),
        }
    }

    fn click_pack() -> RecognitionPack {
        RecognitionPack {
            targets: vec![click_target("tap")],
            ..base_pack()
        }
    }

    fn click_pack_with_id(id: &str) -> RecognitionPack {
        RecognitionPack {
            targets: vec![click_target(id)],
            ..base_pack()
        }
    }

    fn click_target(id: &str) -> RecognitionTarget {
        RecognitionTarget::ClickOnly(ClickOnlyTarget {
            id: id.to_string(),
            click: rect(10, 20, 30, 40),
        })
    }

    fn color_pack(expected: [u8; 3]) -> RecognitionPack {
        RecognitionPack {
            targets: vec![RecognitionTarget::Color(ColorTarget {
                id: "color".to_string(),
                region: rect(0, 0, 20, 20),
                expected,
                click: None,
            })],
            ..base_pack()
        }
    }

    fn rect(x: i32, y: i32, width: i32, height: i32) -> PackRect {
        PackRect {
            x,
            y,
            width,
            height,
        }
    }

    struct TemplateFixture {
        dir: TestDir,
        template: RgbImage,
    }

    impl TemplateFixture {
        fn new() -> Self {
            let dir = TestDir::new();
            let template = template_image();
            dir.write("templates/button.png", &encode_png(&template))
                .expect("write template");
            Self { dir, template }
        }

        fn template_evaluator(&self, threshold: f32) -> RecognitionEvaluator {
            self.template_evaluator_with_defaults(
                RecognitionDefaults {
                    template_threshold: threshold,
                    ..RecognitionDefaults::default()
                },
                None,
            )
        }

        fn template_evaluator_with_defaults(
            &self,
            defaults: RecognitionDefaults,
            target_threshold: Option<f32>,
        ) -> RecognitionEvaluator {
            self.template_evaluator_with_options(
                defaults,
                PackRegion::Rect(rect(12, 10, 28, 24)),
                target_threshold,
            )
        }

        fn template_evaluator_with_region(&self, region: PackRegion) -> RecognitionEvaluator {
            self.template_evaluator_with_options(RecognitionDefaults::default(), region, None)
        }

        fn template_evaluator_with_options(
            &self,
            defaults: RecognitionDefaults,
            region: PackRegion,
            target_threshold: Option<f32>,
        ) -> RecognitionEvaluator {
            let pack = RecognitionPack {
                coordinate_space: Some(PackCoordinateSpace {
                    width: 64,
                    height: 48,
                }),
                defaults,
                targets: vec![RecognitionTarget::Template(TemplateTarget {
                    id: "template".to_string(),
                    template_path: "templates/button.png".to_string(),
                    region,
                    threshold: target_threshold,
                    method: RecognitionMethod::Ncc,
                    mask: None,
                    rect_move: None,
                    color_check: None,
                    click: None,
                })],
                ..base_pack()
            };
            RecognitionEvaluator::new(self.dir.path.clone(), pack).expect("evaluator")
        }

        fn template_with_color_evaluator(&self, expected: [u8; 3]) -> RecognitionEvaluator {
            let pack = RecognitionPack {
                coordinate_space: Some(PackCoordinateSpace {
                    width: 64,
                    height: 48,
                }),
                targets: vec![RecognitionTarget::Template(TemplateTarget {
                    id: "template".to_string(),
                    template_path: "templates/button.png".to_string(),
                    region: PackRegion::Rect(rect(12, 10, 28, 24)),
                    threshold: None,
                    method: RecognitionMethod::Ncc,
                    mask: None,
                    rect_move: None,
                    color_check: Some(ColorCheck {
                        region: rect(0, 0, 8, 8),
                        expected,
                    }),
                    click: None,
                })],
                ..base_pack()
            };
            RecognitionEvaluator::new(self.dir.path.clone(), pack).expect("evaluator")
        }

        fn mixed_vision_pack(&self) -> RecognitionPack {
            RecognitionPack {
                schema_version: "0.6".to_string(),
                coordinate_space: Some(PackCoordinateSpace {
                    width: 64,
                    height: 48,
                }),
                targets: vec![
                    RecognitionTarget::Template(TemplateTarget {
                        id: "template".to_string(),
                        template_path: "templates/button.png".to_string(),
                        region: PackRegion::Rect(rect(12, 10, 28, 24)),
                        threshold: None,
                        method: RecognitionMethod::Ncc,
                        mask: None,
                        rect_move: None,
                        color_check: None,
                        click: None,
                    }),
                    RecognitionTarget::Color(ColorTarget {
                        id: "color".to_string(),
                        region: rect(0, 0, 4, 4),
                        expected: [30, 31, 32],
                        click: None,
                    }),
                    RecognitionTarget::Ocr(OcrTarget {
                        id: "ocr/page".to_string(),
                        region: PackRegion::Rect(rect(0, 0, 2, 1)),
                        languages: vec!["en".to_string()],
                        timeout_ms: 1_000,
                        match_mode: OcrMatchMode::Exact,
                        expected: vec!["hello".to_string()],
                        case_sensitive: false,
                        minimum_confidence: 0.9,
                        model_ref: PPOCR_V6_MEDIUM_MODEL_REF.to_string(),
                        model_sha256: "a".repeat(64),
                        click: None,
                    }),
                    RecognitionTarget::Nn(NnTarget {
                        id: "nn/page".to_string(),
                        region: PackRegion::Rect(rect(0, 0, 2, 1)),
                        model_ref: "fixture-page-model".to_string(),
                        model_sha256: "b".repeat(64),
                        candidate_labels: vec!["home".to_string(), "settings".to_string()],
                        minimum_score: 0.75,
                        selection: NnSelectionMode::Label,
                        expected_label: Some("home".to_string()),
                        timeout_ms: 1_000,
                        click: None,
                    }),
                ],
                ..base_pack()
            }
        }

        fn scene_with_template(&self) -> Scene {
            let mut frame = blank_image(64, 48, [30, 31, 32]);
            paste(&mut frame, &self.template, 20, 15);
            Scene::from_png(&encode_png(&frame)).expect("scene")
        }

        fn blank_scene(&self) -> Scene {
            Scene::from_png(&encode_png(&blank_image(64, 48, [30, 31, 32]))).expect("scene")
        }
    }

    #[derive(Debug)]
    struct TestVisionProvider {
        execution: Option<OcrProviderExecutionEvidence>,
        ocr: Result<OcrProviderResult, VisionProviderError>,
        nn: Result<NnProviderResult, VisionProviderError>,
    }

    impl VisionProvider for TestVisionProvider {
        fn require_ocr_model(
            &self,
            model_ref: &str,
            model_sha256: &str,
        ) -> Result<(), VisionProviderError> {
            if model_ref == PPOCR_V6_MEDIUM_MODEL_REF
                && model_sha256
                    == "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            {
                Ok(())
            } else {
                Err(VisionProviderError::new(
                    VisionProviderErrorCode::ModelMismatch,
                    "unexpected OCR model identity",
                ))
            }
        }

        fn require_nn_model(
            &self,
            model_ref: &str,
            model_sha256: &str,
        ) -> Result<(), VisionProviderError> {
            if model_ref == "fixture-page-model"
                && model_sha256
                    == "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            {
                Ok(())
            } else {
                Err(VisionProviderError::new(
                    VisionProviderErrorCode::ModelMismatch,
                    "unexpected NN model identity",
                ))
            }
        }

        fn read_text(
            &self,
            request: OcrProviderRequest<'_>,
        ) -> Result<OcrProviderResult, VisionProviderError> {
            assert_eq!(request.model_ref, PPOCR_V6_MEDIUM_MODEL_REF);
            assert_eq!(
                request.model_sha256,
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            );
            assert_eq!(request.frame.rgb8_pixels.len(), 6);
            self.ocr.clone()
        }

        fn read_text_with_execution_evidence(
            &self,
            request: OcrProviderRequest<'_>,
        ) -> Result<OcrProviderObservation, VisionProviderError> {
            self.read_text(request)
                .map(|result| OcrProviderObservation {
                    result,
                    execution: self.execution.clone(),
                })
        }

        fn classify(
            &self,
            request: NnProviderRequest<'_>,
        ) -> Result<NnProviderResult, VisionProviderError> {
            assert_eq!(request.model_ref, "fixture-page-model");
            assert_eq!(
                request.model_sha256,
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            );
            assert_eq!(request.frame.rgb8_pixels.len(), 6);
            self.nn.clone()
        }
    }

    #[derive(Debug)]
    struct TrackingVisionProvider {
        reject_models: bool,
        ocr_requirements: AtomicUsize,
        nn_requirements: AtomicUsize,
        ocr_inferences: AtomicUsize,
        nn_inferences: AtomicUsize,
    }

    impl TrackingVisionProvider {
        fn new(reject_models: bool) -> Self {
            Self {
                reject_models,
                ocr_requirements: AtomicUsize::new(0),
                nn_requirements: AtomicUsize::new(0),
                ocr_inferences: AtomicUsize::new(0),
                nn_inferences: AtomicUsize::new(0),
            }
        }

        fn counts(&self) -> (usize, usize, usize, usize) {
            (
                self.ocr_requirements.load(Ordering::SeqCst),
                self.nn_requirements.load(Ordering::SeqCst),
                self.ocr_inferences.load(Ordering::SeqCst),
                self.nn_inferences.load(Ordering::SeqCst),
            )
        }
    }

    impl VisionProvider for TrackingVisionProvider {
        fn require_ocr_model(
            &self,
            model_ref: &str,
            model_sha256: &str,
        ) -> Result<(), VisionProviderError> {
            self.ocr_requirements.fetch_add(1, Ordering::SeqCst);
            if self.reject_models
                || model_ref != PPOCR_V6_MEDIUM_MODEL_REF
                || model_sha256
                    != "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            {
                Err(VisionProviderError::new(
                    VisionProviderErrorCode::ModelMismatch,
                    "OCR model mismatch",
                ))
            } else {
                Ok(())
            }
        }

        fn require_nn_model(
            &self,
            model_ref: &str,
            model_sha256: &str,
        ) -> Result<(), VisionProviderError> {
            self.nn_requirements.fetch_add(1, Ordering::SeqCst);
            if self.reject_models
                || model_ref != "fixture-page-model"
                || model_sha256
                    != "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            {
                Err(VisionProviderError::new(
                    VisionProviderErrorCode::ModelMismatch,
                    "NN model mismatch",
                ))
            } else {
                Ok(())
            }
        }

        fn read_text(
            &self,
            _request: OcrProviderRequest<'_>,
        ) -> Result<OcrProviderResult, VisionProviderError> {
            self.ocr_inferences.fetch_add(1, Ordering::SeqCst);
            Ok(OcrProviderResult {
                text: "hello".to_string(),
                blocks: vec![OcrProviderTextBlock {
                    text: "hello".to_string(),
                    rect: rect(0, 0, 2, 1),
                    confidence: Some(1.0),
                }],
                confidence: Some(1.0),
            })
        }

        fn classify(
            &self,
            _request: NnProviderRequest<'_>,
        ) -> Result<NnProviderResult, VisionProviderError> {
            self.nn_inferences.fetch_add(1, Ordering::SeqCst);
            Ok(NnProviderResult {
                labels: vec![NnProviderLabel {
                    label: "home".to_string(),
                    score: 0.9,
                }],
            })
        }
    }

    fn vision_evaluator(json: String, provider: Arc<dyn VisionProvider>) -> RecognitionEvaluator {
        let pack = load_pack_from_json_str(&json).expect("vision pack parses");
        RecognitionEvaluator::with_vision_provider(
            pack,
            Arc::new(FsAssetResolver::new(PathBuf::new())),
            provider,
        )
        .expect("vision evaluator")
    }

    fn execution_evidence(provider: OcrExecutionProviderKind) -> OcrProviderExecutionEvidence {
        let cuda = provider == OcrExecutionProviderKind::Cuda;
        OcrProviderExecutionEvidence {
            invocation_id: "fixture-invocation".to_string(),
            session_id: "fixture-session".to_string(),
            session_generation: 1,
            requested_provider: provider,
            resolved_provider: provider,
            requested_cuda_ordinal: cuda.then_some(0),
            requested_cuda_identity: cuda.then(|| "fixture-cuda-0".to_string()),
            resolved_cuda_ordinal: cuda.then_some(0),
            resolved_cuda_identity: cuda.then(|| "fixture-cuda-0".to_string()),
            provider_implementation: "fixture-ocr".to_string(),
            provider_binary_sha256: "b".repeat(64),
            runtime_version: "fixture-runtime".to_string(),
            model_ref: PPOCR_V6_MEDIUM_MODEL_REF.to_string(),
            model_sha256: "a".repeat(64),
            cpu_ep_registered: !cuda,
            cpu_fallback_disabled: cuda,
            fallback_forbidden: true,
            fallback_observed: None,
            complete: true,
        }
    }

    fn observation_evaluator(execution: OcrProviderExecutionEvidence) -> RecognitionEvaluator {
        vision_evaluator(
            ocr_pack_json("exact", "hello", 0.0),
            Arc::new(TestVisionProvider {
                execution: Some(execution),
                ocr: Ok(OcrProviderResult {
                    text: "hello".to_string(),
                    blocks: vec![OcrProviderTextBlock {
                        text: "hello".to_string(),
                        rect: rect(0, 0, 2, 1),
                        confidence: Some(1.0),
                    }],
                    confidence: Some(1.0),
                }),
                nn: Err(VisionProviderError::new(
                    VisionProviderErrorCode::Unavailable,
                    "unused",
                )),
            }),
        )
    }

    fn ocr_pack_json(match_mode: &str, expected: &str, minimum_confidence: f32) -> String {
        format!(
            r#"{{
                "schema_version": "0.6",
                "coordinate_space": {{"width": 2, "height": 1}},
                "targets": [{{
                    "type": "ocr",
                    "id": "ocr/page",
                    "region": "full_frame",
                    "languages": ["en"],
                    "timeout_ms": 1000,
                    "match_mode": "{match_mode}",
                    "expected": ["{expected}"],
                    "case_sensitive": false,
                    "minimum_confidence": {minimum_confidence},
                    "model_ref": "PP-OCRv6_medium",
                    "model_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                }}]
            }}"#
        )
    }

    fn nn_pack_json() -> String {
        r#"{
            "schema_version": "0.6",
            "coordinate_space": {"width": 2, "height": 1},
            "targets": [{
                "type": "nn",
                "id": "nn/page",
                "region": "full_frame",
                "model_ref": "fixture-page-model",
                "model_sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "candidate_labels": ["home", "settings"],
                "minimum_score": 0.75,
                "selection": "label",
                "expected_label": "home",
                "timeout_ms": 1000
            }]
        }"#
        .to_string()
    }

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos();
            let sequence = TEST_DIR_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "actingcommand-recognition-pack-{}-{unique}-{sequence}",
                std::process::id(),
            ));
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        fn write(&self, relative: &str, bytes: &[u8]) -> io::Result<()> {
            let path = self.path.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(path, bytes)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[derive(Clone)]
    struct RgbImage {
        width: u32,
        height: u32,
        pixels: Vec<[u8; 3]>,
    }

    fn blank_image(width: u32, height: u32, color: [u8; 3]) -> RgbImage {
        RgbImage {
            width,
            height,
            pixels: vec![color; (width * height) as usize],
        }
    }

    fn template_image() -> RgbImage {
        let mut image = blank_image(8, 6, [0, 0, 0]);
        for y in 0..image.height {
            for x in 0..image.width {
                image.set(
                    x,
                    y,
                    [
                        ((x * 17 + y * 7) % 251) as u8,
                        ((x * 11 + y * 19 + 23) % 239) as u8,
                        ((x * 5 + y * 29 + 41) % 227) as u8,
                    ],
                );
            }
        }
        image
    }

    fn red_scene() -> Scene {
        Scene::from_png(&encode_png(&blank_image(20, 20, [255, 0, 0]))).expect("scene")
    }

    fn paste(frame: &mut RgbImage, template: &RgbImage, x_offset: u32, y_offset: u32) {
        for y in 0..template.height {
            for x in 0..template.width {
                frame.set(x_offset + x, y_offset + y, template.get(x, y));
            }
        }
    }

    impl RgbImage {
        fn get(&self, x: u32, y: u32) -> [u8; 3] {
            self.pixels[(y * self.width + x) as usize]
        }

        fn set(&mut self, x: u32, y: u32, value: [u8; 3]) {
            self.pixels[(y * self.width + x) as usize] = value;
        }
    }

    fn encode_png(image: &RgbImage) -> Vec<u8> {
        let mut scanlines = Vec::with_capacity((image.width * image.height * 3) as usize);
        for y in 0..image.height {
            scanlines.push(0);
            for x in 0..image.width {
                scanlines.extend_from_slice(&image.get(x, y));
            }
        }

        let mut png = Vec::new();
        png.extend_from_slice(b"\x89PNG\r\n\x1a\n");

        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&image.width.to_be_bytes());
        ihdr.extend_from_slice(&image.height.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        write_chunk(&mut png, b"IHDR", &ihdr);

        let mut zlib = vec![0x78, 0x01];
        write_uncompressed_deflate(&mut zlib, &scanlines);
        zlib.extend_from_slice(&adler32(&scanlines).to_be_bytes());
        write_chunk(&mut png, b"IDAT", &zlib);
        write_chunk(&mut png, b"IEND", &[]);
        png
    }

    fn write_uncompressed_deflate(out: &mut Vec<u8>, data: &[u8]) {
        for (index, chunk) in data.chunks(65_535).enumerate() {
            let is_last = index == data.len().div_ceil(65_535) - 1;
            out.push(u8::from(is_last));
            let len = chunk.len() as u16;
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&(!len).to_le_bytes());
            out.extend_from_slice(chunk);
        }
    }

    fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let mut crc_input = Vec::with_capacity(kind.len() + data.len());
        crc_input.extend_from_slice(kind);
        crc_input.extend_from_slice(data);
        out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    }

    fn adler32(data: &[u8]) -> u32 {
        const MOD: u32 = 65_521;
        let mut a = 1_u32;
        let mut b = 0_u32;
        for byte in data {
            a = (a + u32::from(*byte)) % MOD;
            b = (b + a) % MOD;
        }
        (b << 16) | a
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xffff_ffff_u32;
        for byte in data {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                let mask = 0_u32.wrapping_sub(crc & 1);
                crc = (crc >> 1) ^ (0xedb8_8320 & mask);
            }
        }
        !crc
    }

    fn assert_fatal_contains(err: RecognitionPackError, needle: &str) {
        assert_eq!(err.severity(), RecognitionPackErrorSeverity::Fatal);
        assert!(
            err.message().contains(needle),
            "expected '{needle}' in '{}'",
            err.message()
        );
    }
}
