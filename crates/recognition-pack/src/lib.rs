// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_recognition as recognition;
use recognition::{MatchMetric, Scene};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub type RecognitionPackResult<T> = Result<T, RecognitionPackError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecognitionPackErrorSeverity {
    Fatal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecognitionPackError {
    severity: RecognitionPackErrorSeverity,
    message: String,
}

impl RecognitionPackError {
    pub fn fatal(message: impl Into<String>) -> Self {
        Self {
            severity: RecognitionPackErrorSeverity::Fatal,
            message: message.into(),
        }
    }

    pub fn severity(&self) -> RecognitionPackErrorSeverity {
        self.severity
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct PackPoint {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum PackRegion {
    Rect(PackRect),
    Keyword(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
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

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct ColorCheck {
    pub region: PackRect,
    pub expected: [u8; 3],
}

#[derive(Debug, Clone)]
pub struct RecognitionEvaluator {
    asset_resolver: Arc<dyn AssetResolver>,
    pack: RecognitionPack,
    target_indexes: HashMap<String, usize>,
    unsupported_targets: Vec<UnsupportedRecognitionTarget>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    Template,
    Color,
    ClickOnly,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TargetEvaluation {
    pub id: String,
    pub kind: TargetKind,
    pub passed: bool,
    pub template: Option<TemplateEvaluation>,
    pub color: Option<ColorEvaluation>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TemplateEvaluation {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub raw_score: f32,
    pub score: f32,
    pub threshold: f32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedRecognitionTarget {
    pub id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorEvaluation {
    pub distance: f32,
    pub max_distance: f32,
    pub mean: [u8; 3],
    pub expected: [u8; 3],
}

pub fn load_pack_from_json_str(json: &str) -> RecognitionPackResult<RecognitionPack> {
    serde_json::from_str(json).map_err(|err| {
        RecognitionPackError::fatal(format!("failed to parse recognition pack JSON: {err}"))
    })
}

impl RecognitionEvaluator {
    pub fn new(pack_root: PathBuf, pack: RecognitionPack) -> RecognitionPackResult<Self> {
        Self::with_asset_resolver(pack, Arc::new(FsAssetResolver::new(pack_root)))
    }

    pub fn with_asset_resolver(
        pack: RecognitionPack,
        asset_resolver: Arc<dyn AssetResolver>,
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
        }
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
            RecognitionTarget::Color(_) | RecognitionTarget::ClickOnly(_) => Ok(None),
        }
    }

    pub fn target_kind(&self, target_id: &str) -> RecognitionPackResult<TargetKind> {
        let target = self.target(target_id)?;
        Ok(match target {
            RecognitionTarget::Template(_) => TargetKind::Template,
            RecognitionTarget::Color(_) => TargetKind::Color,
            RecognitionTarget::ClickOnly(_) => TargetKind::ClickOnly,
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
            message: if passed {
                "color passed".to_string()
            } else {
                "color failed".to_string()
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
            RecognitionTarget::Color(_) | RecognitionTarget::ClickOnly(_) => None,
        })
        .collect()
}

impl RecognitionTarget {
    fn id(&self) -> &str {
        match self {
            Self::Template(target) => &target.id,
            Self::Color(target) => &target.id,
            Self::ClickOnly(target) => &target.id,
        }
    }
}

fn validate_pack(
    asset_resolver: &dyn AssetResolver,
    pack: &RecognitionPack,
    errors: &mut Vec<String>,
) {
    if !matches!(pack.schema_version.as_str(), "0.1" | "0.3" | "0.4" | "0.5") {
        errors.push(format!(
            "unsupported schema_version '{}', expected one of '0.1', '0.3', '0.4', '0.5'",
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
    use std::sync::atomic::{AtomicU64, Ordering};
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

        fn scene_with_template(&self) -> Scene {
            let mut frame = blank_image(64, 48, [30, 31, 32]);
            paste(&mut frame, &self.template, 20, 15);
            Scene::from_png(&encode_png(&frame)).expect("scene")
        }

        fn blank_scene(&self) -> Scene {
            Scene::from_png(&encode_png(&blank_image(64, 48, [30, 31, 32]))).expect("scene")
        }
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
