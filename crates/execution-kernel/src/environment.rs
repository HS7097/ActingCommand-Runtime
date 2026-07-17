// SPDX-License-Identifier: AGPL-3.0-only

//! Pure environment-result validation and marker-resolution decisions.

use actingcommand_contract::{
    EnvDetected, EnvResolved, FactContent, FactResolution, FactUnknownReason, FactValue,
    InstanceFactSnapshot,
};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::path::Path;

pub const ENV_RESULT_SCHEMA_VERSION: &str = "env-detect-result.v1";
const ENV_DETECTION_MAX_STEP_DURATION_MS: u64 = 60_000;

pub type EnvironmentStateResult<T> = Result<T, EnvironmentStateError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvironmentStateErrorKind {
    SchemaMismatch,
    InstanceMismatch,
    ScopeMismatch,
    DetectorMismatch,
    ResourceHashChanged,
    MissingKey,
    LowConfidence,
    Expired,
    UnsafeValue,
    UnallowedValue,
    InvalidPointer,
    UndeclaredKey,
}

impl EnvironmentStateErrorKind {
    pub fn reason(self) -> &'static str {
        match self {
            Self::SchemaMismatch => "schema_mismatch",
            Self::InstanceMismatch => "instance_mismatch",
            Self::ScopeMismatch => "scope_mismatch",
            Self::DetectorMismatch => "detector_mismatch",
            Self::ResourceHashChanged => "resource_hash_changed",
            Self::MissingKey => "missing_key",
            Self::LowConfidence => "low_confidence",
            Self::Expired => "expired",
            Self::UnsafeValue => "unsafe_value",
            Self::UnallowedValue => "unallowed_value",
            Self::InvalidPointer => "invalid_pointer",
            Self::UndeclaredKey => "undeclared_key",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentStateError {
    kind: EnvironmentStateErrorKind,
    message: String,
}

impl EnvironmentStateError {
    fn new(kind: EnvironmentStateErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn kind(&self) -> EnvironmentStateErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for EnvironmentStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "environment state error: {}", self.message)
    }
}

impl Error for EnvironmentStateError {}

pub type EnvironmentCatalogResult<T> = Result<T, EnvironmentCatalogError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentCatalogError {
    message: String,
}

impl EnvironmentCatalogError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for EnvironmentCatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "environment catalog error: {}", self.message)
    }
}

impl Error for EnvironmentCatalogError {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvDetectionCatalog {
    #[serde(default)]
    pub schema_version: Option<String>,
    #[serde(default, alias = "detectors", alias = "tasks")]
    pub detections: Vec<EnvDetector>,
}

impl EnvDetectionCatalog {
    pub fn detector(&self, id: &str) -> EnvironmentCatalogResult<&EnvDetector> {
        self.detections
            .iter()
            .find(|detector| detector.id == id)
            .ok_or_else(|| {
                EnvironmentCatalogError::new(format!("env detector '{id}' was not found"))
            })
    }

    pub fn select_detector_for_keys(
        &self,
        requested_detector: Option<&str>,
        keys: &BTreeSet<String>,
    ) -> EnvironmentCatalogResult<&EnvDetector> {
        if let Some(detector_id) = requested_detector {
            let detector = self.detector(detector_id)?;
            if detector.declares_keys(keys) {
                return Ok(detector);
            }
            return Err(EnvironmentCatalogError::new(format!(
                "env detector '{}' does not declare all required keys: {}",
                detector.id,
                joined_keys(keys)
            )));
        }
        let matches = self
            .detections
            .iter()
            .filter(|detector| detector.declares_keys(keys))
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [detector] => Ok(detector),
            [] => Err(EnvironmentCatalogError::new(format!(
                "no env detector declares all required keys: {}",
                joined_keys(keys)
            ))),
            _ => Err(EnvironmentCatalogError::new(format!(
                "env keys are ambiguous across detectors; pass --env-task explicitly for keys: {}",
                joined_keys(keys)
            ))),
        }
    }

    pub fn validate(&self) -> EnvironmentCatalogResult<()> {
        if let Some(schema_version) = &self.schema_version
            && schema_version != "env-detection.v1"
            && schema_version != "env-detections.v1"
        {
            return Err(EnvironmentCatalogError::new(format!(
                "unsupported env detection schema_version '{schema_version}'"
            )));
        }
        let mut detector_ids = BTreeSet::new();
        for detector in &self.detections {
            if detector.id.trim().is_empty() {
                return Err(EnvironmentCatalogError::new(
                    "env detector id must not be empty",
                ));
            }
            if !detector_ids.insert(detector.id.clone()) {
                return Err(EnvironmentCatalogError::new(format!(
                    "env detector id '{}' is duplicated",
                    detector.id
                )));
            }
            if detector.keys.is_empty() {
                return Err(EnvironmentCatalogError::new(format!(
                    "env detector '{}' must declare at least one key",
                    detector.id
                )));
            }
            for (index, step) in detector.steps.iter().enumerate() {
                detector.validate_step(index, step)?;
            }
            let mut key_ids = BTreeSet::new();
            for key in &detector.keys {
                validate_detection_key(detector, key)?;
                if !key_ids.insert(key.key.clone()) {
                    return Err(EnvironmentCatalogError::new(format!(
                        "env detector '{}' key '{}' is duplicated",
                        detector.id, key.key
                    )));
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvDetector {
    #[serde(alias = "task_id", alias = "detector_id")]
    pub id: String,
    #[serde(default, alias = "detector_version")]
    pub version: Option<String>,
    #[serde(default, alias = "game")]
    pub game_id: Option<String>,
    #[serde(default, alias = "server")]
    pub server_id: Option<String>,
    #[serde(default)]
    pub resource_pack_id: Option<String>,
    #[serde(default)]
    pub match_metric: Option<String>,
    #[serde(default, alias = "actions", alias = "pre_actions", alias = "pre_steps")]
    pub steps: Vec<EnvDetectionStep>,
    #[serde(alias = "outputs", alias = "items")]
    pub keys: Vec<EnvDetectionKey>,
}

impl EnvDetector {
    pub fn version(&self) -> &str {
        self.version.as_deref().unwrap_or("1")
    }

    pub fn resource_pack_id(&self, game_id: &str, server_id: &str) -> String {
        self.resource_pack_id
            .clone()
            .unwrap_or_else(|| format!("{game_id}.{server_id}"))
    }

    pub fn validate_scope(&self, game_id: &str, server_id: &str) -> EnvironmentCatalogResult<()> {
        if let Some(game) = &self.game_id {
            let game = canonical_environment_game(game)?;
            if game != game_id {
                return Err(EnvironmentCatalogError::new(format!(
                    "env detector '{}' is scoped to game '{}' but command game is '{}'",
                    self.id, game, game_id
                )));
            }
        }
        if let Some(server) = &self.server_id
            && server != server_id
        {
            return Err(EnvironmentCatalogError::new(format!(
                "env detector '{}' is scoped to server '{}' but command server is '{}'",
                self.id, server, server_id
            )));
        }
        Ok(())
    }

    pub fn validate_step(
        &self,
        index: usize,
        step: &EnvDetectionStep,
    ) -> EnvironmentCatalogResult<()> {
        step.plan().map_err(|error| {
            EnvironmentCatalogError::new(format!(
                "env detector '{}' step {} is invalid: {}",
                self.id,
                index + 1,
                error.message()
            ))
        })?;
        Ok(())
    }

    fn declares_keys(&self, keys: &BTreeSet<String>) -> bool {
        keys.iter()
            .all(|key| self.keys.iter().any(|item| &item.key == key))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvDetectionStep {
    #[serde(alias = "type", alias = "action")]
    pub kind: String,
    #[serde(default)]
    pub x: Option<i32>,
    #[serde(default)]
    pub y: Option<i32>,
    #[serde(default)]
    pub x1: Option<i32>,
    #[serde(default)]
    pub y1: Option<i32>,
    #[serde(default)]
    pub x2: Option<i32>,
    #[serde(default)]
    pub y2: Option<i32>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
}

impl EnvDetectionStep {
    pub fn plan(&self) -> EnvironmentCatalogResult<EnvDetectionStepPlan> {
        Ok(match self.canonical_kind()? {
            "tap" => EnvDetectionStepPlan::Tap {
                x: self.required_coord("x")?,
                y: self.required_coord("y")?,
            },
            "long_tap" => EnvDetectionStepPlan::LongTap {
                x: self.required_coord("x")?,
                y: self.required_coord("y")?,
                duration_ms: self.required_duration()?,
            },
            "swipe" => EnvDetectionStepPlan::Swipe {
                x1: self.required_coord("x1")?,
                y1: self.required_coord("y1")?,
                x2: self.required_coord("x2")?,
                y2: self.required_coord("y2")?,
                duration_ms: self.required_duration()?,
            },
            "wait" => EnvDetectionStepPlan::Wait {
                duration_ms: self.required_duration()?,
            },
            _ => unreachable!(),
        })
    }

    pub fn required_duration(&self) -> EnvironmentCatalogResult<u64> {
        let duration_ms = self.duration_ms.ok_or_else(|| {
            EnvironmentCatalogError::new(format!(
                "env detection step '{}' is missing duration_ms",
                self.kind
            ))
        })?;
        if duration_ms == 0 || duration_ms > ENV_DETECTION_MAX_STEP_DURATION_MS {
            return Err(EnvironmentCatalogError::new(format!(
                "env detection step '{}' duration_ms must be in 1..={ENV_DETECTION_MAX_STEP_DURATION_MS}",
                self.kind
            )));
        }
        Ok(duration_ms)
    }

    fn canonical_kind(&self) -> EnvironmentCatalogResult<&'static str> {
        match self.kind.trim() {
            "tap" => Ok("tap"),
            "long_tap" | "long-tap" | "longtap" => Ok("long_tap"),
            "swipe" => Ok("swipe"),
            "wait" | "sleep" => Ok("wait"),
            other => Err(EnvironmentCatalogError::new(format!(
                "unsupported env detection step kind '{other}'"
            ))),
        }
    }

    fn required_coord(&self, name: &str) -> EnvironmentCatalogResult<i32> {
        let value = match name {
            "x" => self.x,
            "y" => self.y,
            "x1" => self.x1,
            "y1" => self.y1,
            "x2" => self.x2,
            "y2" => self.y2,
            _ => None,
        }
        .ok_or_else(|| {
            EnvironmentCatalogError::new(format!(
                "env detection step '{}' is missing coordinate {name}",
                self.kind
            ))
        })?;
        if value < 0 {
            return Err(EnvironmentCatalogError::new(format!(
                "env detection step '{}' coordinate {name} must be non-negative",
                self.kind
            )));
        }
        Ok(value)
    }
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvDetectionKey {
    pub key: String,
    #[serde(alias = "threshold")]
    pub min_confidence: f32,
    #[serde(default, alias = "invalidate_below_confidence")]
    pub stale_below_confidence: Option<f32>,
    #[serde(default)]
    pub ttl_ms: Option<u64>,
    pub allowed_values: Vec<String>,
    pub candidates: Vec<EnvDetectionCandidate>,
}

impl EnvDetectionKey {
    pub fn stale_threshold(&self) -> f32 {
        self.stale_below_confidence.unwrap_or(self.min_confidence)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvDetectionCandidate {
    pub value: String,
    #[serde(default, alias = "template")]
    pub template_path: Option<String>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(
        default,
        alias = "roi",
        deserialize_with = "deserialize_env_rect_option"
    )]
    pub region: Option<EnvRect>,
    #[serde(default)]
    pub threshold: Option<f32>,
    #[serde(default)]
    pub source: Option<String>,
}

impl EnvDetectionCandidate {
    pub fn matcher(&self, key: &str) -> EnvironmentCatalogResult<EnvCandidateMatcher> {
        let template = self.template_path.as_deref().map(str::trim);
        let has_template = template.is_some_and(|value| !value.is_empty());
        let has_empty_template = template.is_some_and(str::is_empty);
        let has_width = self.width.is_some();
        let has_height = self.height.is_some();
        let has_scene_size = has_width || has_height;

        if has_empty_template {
            return Err(EnvironmentCatalogError::new(format!(
                "env key '{}' candidate '{}' has empty template_path",
                key, self.value
            )));
        }
        if has_template && has_scene_size {
            return Err(EnvironmentCatalogError::new(format!(
                "env key '{}' candidate '{}' must not mix template and scene size matchers",
                key, self.value
            )));
        }
        if has_template {
            return Ok(EnvCandidateMatcher::Template);
        }
        if has_width && has_height {
            let width = self.width.unwrap_or_default();
            let height = self.height.unwrap_or_default();
            if width == 0 || height == 0 {
                return Err(EnvironmentCatalogError::new(format!(
                    "env key '{}' candidate '{}' scene size must be non-zero",
                    key, self.value
                )));
            }
            return Ok(EnvCandidateMatcher::SceneSize { width, height });
        }
        if has_scene_size {
            return Err(EnvironmentCatalogError::new(format!(
                "env key '{}' candidate '{}' scene size matcher requires width and height",
                key, self.value
            )));
        }
        Err(EnvironmentCatalogError::new(format!(
            "env key '{}' candidate '{}' must declare template_path or width/height",
            key, self.value
        )))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvCandidateMatcher {
    Template,
    SceneSize { width: u32, height: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

pub fn parse_environment_catalog_value(
    value: Value,
) -> EnvironmentCatalogResult<EnvDetectionCatalog> {
    match serde_json::from_value::<EnvDetectionCatalog>(value.clone()) {
        Ok(catalog) => Ok(catalog),
        Err(structured_error) => normalize_flat_environment_catalog(value).map_err(|flat_error| {
            EnvironmentCatalogError::new(format!(
                "structured parse failed: {structured_error}; flat parse failed: {}",
                flat_error.message()
            ))
        }),
    }
}

pub fn canonical_environment_game(value: &str) -> EnvironmentCatalogResult<String> {
    let selector = value.trim().to_ascii_lowercase();
    if selector.is_empty()
        || selector.len() > 128
        || !selector
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(EnvironmentCatalogError::new(format!(
            "invalid environment game selector: {value}"
        )));
    }
    Ok(selector)
}

pub fn default_environment_server(_game: &str) -> &'static str {
    "default"
}

#[derive(Debug, Deserialize)]
struct FlatEnvDetectionCatalog {
    #[serde(default)]
    schema_version: Option<String>,
    #[serde(default, alias = "game_id")]
    game: Option<String>,
    #[serde(default, alias = "server_id")]
    server: Option<String>,
    #[serde(default)]
    resource_pack_id: Option<String>,
    #[serde(default)]
    match_metric: Option<String>,
    #[serde(default)]
    detections: Vec<FlatEnvDetectionItem>,
}

#[derive(Debug, Deserialize)]
struct FlatEnvDetectionItem {
    #[serde(alias = "id", alias = "task_id")]
    detector_id: String,
    #[serde(default, alias = "version")]
    detector_version: Option<String>,
    #[serde(default, alias = "game", alias = "game_id")]
    game_id: Option<String>,
    #[serde(default, alias = "server", alias = "server_id")]
    server_id: Option<String>,
    #[serde(default)]
    resource_pack_id: Option<String>,
    #[serde(default)]
    match_metric: Option<String>,
    #[serde(default, alias = "actions", alias = "pre_actions", alias = "pre_steps")]
    steps: Vec<EnvDetectionStep>,
    #[serde(flatten)]
    key: EnvDetectionKey,
}

fn normalize_flat_environment_catalog(
    value: Value,
) -> EnvironmentCatalogResult<EnvDetectionCatalog> {
    let flat: FlatEnvDetectionCatalog = serde_json::from_value(value).map_err(|error| {
        EnvironmentCatalogError::new(format!("invalid flat env detection catalog: {error}"))
    })?;
    let mut detectors = BTreeMap::<String, EnvDetector>::new();
    for item in flat.detections {
        let detector_id = item.detector_id.trim().to_string();
        if detector_id.is_empty() {
            return Err(EnvironmentCatalogError::new(
                "flat env detection item has an empty detector_id",
            ));
        }
        let candidate = EnvDetector {
            id: detector_id.clone(),
            version: item.detector_version,
            game_id: item.game_id.or_else(|| flat.game.clone()),
            server_id: item.server_id.or_else(|| flat.server.clone()),
            resource_pack_id: item
                .resource_pack_id
                .or_else(|| flat.resource_pack_id.clone()),
            match_metric: item.match_metric.or_else(|| flat.match_metric.clone()),
            steps: item.steps,
            keys: Vec::new(),
        };
        let detector = detectors
            .entry(detector_id)
            .or_insert_with(|| candidate.clone());
        ensure_flat_detector_metadata_matches(detector, &candidate)?;
        detector.keys.push(item.key);
    }
    Ok(EnvDetectionCatalog {
        schema_version: flat.schema_version,
        detections: detectors.into_values().collect(),
    })
}

fn ensure_flat_detector_metadata_matches(
    current: &EnvDetector,
    candidate: &EnvDetector,
) -> EnvironmentCatalogResult<()> {
    let fields = [
        (
            "version",
            current.version.as_ref(),
            candidate.version.as_ref(),
        ),
        (
            "game_id",
            current.game_id.as_ref(),
            candidate.game_id.as_ref(),
        ),
        (
            "server_id",
            current.server_id.as_ref(),
            candidate.server_id.as_ref(),
        ),
        (
            "resource_pack_id",
            current.resource_pack_id.as_ref(),
            candidate.resource_pack_id.as_ref(),
        ),
        (
            "match_metric",
            current.match_metric.as_ref(),
            candidate.match_metric.as_ref(),
        ),
    ];
    for (field, left, right) in fields {
        if left != right {
            return Err(EnvironmentCatalogError::new(format!(
                "flat env detector '{}' has conflicting {field}",
                current.id
            )));
        }
    }
    if current.steps != candidate.steps {
        return Err(EnvironmentCatalogError::new(format!(
            "flat env detector '{}' has conflicting steps",
            current.id
        )));
    }
    Ok(())
}

fn validate_detection_key(
    detector: &EnvDetector,
    key: &EnvDetectionKey,
) -> EnvironmentCatalogResult<()> {
    if key.key.trim().is_empty() {
        return Err(EnvironmentCatalogError::new(format!(
            "env detector '{}' has an empty key",
            detector.id
        )));
    }
    validate_confidence(key.min_confidence, &format!("{}.min_confidence", key.key))?;
    if let Some(threshold) = key.stale_below_confidence {
        validate_confidence(threshold, &format!("{}.stale_below_confidence", key.key))?;
    }
    if key.allowed_values.is_empty() {
        return Err(EnvironmentCatalogError::new(format!(
            "env key '{}' must declare allowed_values",
            key.key
        )));
    }
    let allowed = key.allowed_values.iter().cloned().collect::<BTreeSet<_>>();
    if allowed.len() != key.allowed_values.len() {
        return Err(EnvironmentCatalogError::new(format!(
            "env key '{}' allowed_values contains duplicate entries",
            key.key
        )));
    }
    for value in &key.allowed_values {
        validate_environment_value_safety(value, &key.key)
            .map_err(|error| EnvironmentCatalogError::new(error.message()))?;
    }
    if key.candidates.is_empty() {
        return Err(EnvironmentCatalogError::new(format!(
            "env key '{}' must declare candidates",
            key.key
        )));
    }
    for candidate in &key.candidates {
        validate_environment_value_safety(&candidate.value, &key.key)
            .map_err(|error| EnvironmentCatalogError::new(error.message()))?;
        if !key
            .allowed_values
            .iter()
            .any(|allowed| allowed == &candidate.value)
        {
            return Err(EnvironmentCatalogError::new(format!(
                "env key '{}' candidate value '{}' is not in allowed_values",
                key.key, candidate.value
            )));
        }
        candidate.matcher(&key.key)?;
        if let Some(threshold) = candidate.threshold {
            validate_confidence(threshold, &format!("{}.candidate.threshold", key.key))?;
        }
    }
    Ok(())
}

fn validate_confidence(value: f32, label: &str) -> EnvironmentCatalogResult<()> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        return Ok(());
    }
    Err(EnvironmentCatalogError::new(format!(
        "{label} must be finite and in 0.0..=1.0"
    )))
}

fn deserialize_env_rect_option<'de, D>(deserializer: D) -> Result<Option<EnvRect>, D::Error>
where
    D: Deserializer<'de>,
{
    let Some(value) = Option::<Value>::deserialize(deserializer)? else {
        return Ok(None);
    };
    env_rect_from_value(&value).map_err(serde::de::Error::custom)
}

fn env_rect_from_value(value: &Value) -> Result<Option<EnvRect>, String> {
    match value {
        Value::Null => Ok(None),
        Value::String(text) if text == "full_frame" => Ok(None),
        Value::Object(_) => {
            let rect: EnvRect = serde_json::from_value(value.clone())
                .map_err(|error| format!("invalid env rect object: {error}"))?;
            Ok(Some(rect))
        }
        Value::Array(values) => {
            if values.len() != 4 {
                return Err("env roi array must contain exactly [x, y, width, height]".to_string());
            }
            let mut numbers = [0i32; 4];
            for (index, value) in values.iter().enumerate() {
                let number = value
                    .as_i64()
                    .ok_or_else(|| "env roi values must be integers".to_string())?;
                numbers[index] = i32::try_from(number)
                    .map_err(|_| "env roi value is outside i32 range".to_string())?;
            }
            Ok(Some(EnvRect {
                x: numbers[0],
                y: numbers[1],
                width: numbers[2],
                height: numbers[3],
            }))
        }
        _ => Err("env region must be an object, null, full_frame, or roi array".to_string()),
    }
}

fn joined_keys(keys: &BTreeSet<String>) -> String {
    keys.iter().cloned().collect::<Vec<_>>().join(", ")
}

pub type EnvironmentDecisionResult<T> = Result<T, EnvironmentDecisionError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentDecisionError {
    message: String,
}

impl EnvironmentDecisionError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for EnvironmentDecisionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "environment decision error: {}", self.message)
    }
}

impl Error for EnvironmentDecisionError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentDetectionContext {
    pub instance_id: String,
    pub game_id: String,
    pub server_id: String,
    pub resource_pack_hash: String,
    pub generated_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnvironmentCandidateObservation {
    pub key: String,
    pub candidate_index: usize,
    pub confidence: f32,
}

/// Selects environment candidates from complete caller-supplied recognition observations.
pub struct EnvironmentDetectionEngine;

impl EnvironmentDetectionEngine {
    pub fn decide(
        detector: &EnvDetector,
        context: &EnvironmentDetectionContext,
        observations: Vec<EnvironmentCandidateObservation>,
    ) -> EnvironmentDecisionResult<EnvDetectionResult> {
        detector
            .validate_scope(&context.game_id, &context.server_id)
            .map_err(|error| EnvironmentDecisionError::new(error.message()))?;
        let observation_map = validate_candidate_observations(detector, observations)?;
        let mut detections = BTreeMap::new();
        for key in &detector.keys {
            let value = decide_detection_key(detector, key, context, &observation_map)?;
            detections.insert(key.key.clone(), value);
        }
        Ok(EnvDetectionResult {
            schema_version: ENV_RESULT_SCHEMA_VERSION.to_string(),
            instance_id: context.instance_id.clone(),
            game_id: context.game_id.clone(),
            server_id: context.server_id.clone(),
            detector_id: detector.id.clone(),
            detector_version: detector.version().to_string(),
            resource_pack_id: detector.resource_pack_id(&context.game_id, &context.server_id),
            resource_pack_hash: context.resource_pack_hash.clone(),
            generated_at_unix_ms: context.generated_at_unix_ms,
            detections,
        })
    }
}

fn validate_candidate_observations(
    detector: &EnvDetector,
    observations: Vec<EnvironmentCandidateObservation>,
) -> EnvironmentDecisionResult<BTreeMap<(String, usize), f32>> {
    let mut observation_map = BTreeMap::new();
    for observation in observations {
        let key = detector
            .keys
            .iter()
            .find(|key| key.key == observation.key)
            .ok_or_else(|| {
                EnvironmentDecisionError::new(format!(
                    "environment observation references unknown key '{}'",
                    observation.key
                ))
            })?;
        if observation.candidate_index >= key.candidates.len() {
            return Err(EnvironmentDecisionError::new(format!(
                "environment observation for key '{}' references missing candidate {}",
                observation.key, observation.candidate_index
            )));
        }
        if !observation.confidence.is_finite() || !(0.0..=1.0).contains(&observation.confidence) {
            return Err(EnvironmentDecisionError::new(format!(
                "environment observation for key '{}' candidate {} has invalid confidence {}",
                observation.key, observation.candidate_index, observation.confidence
            )));
        }
        let identity = (observation.key.clone(), observation.candidate_index);
        if observation_map
            .insert(identity, observation.confidence)
            .is_some()
        {
            return Err(EnvironmentDecisionError::new(format!(
                "environment observation for key '{}' candidate {} is duplicated",
                observation.key, observation.candidate_index
            )));
        }
    }
    for key in &detector.keys {
        for index in 0..key.candidates.len() {
            if !observation_map.contains_key(&(key.key.clone(), index)) {
                return Err(EnvironmentDecisionError::new(format!(
                    "environment observation for key '{}' candidate {} is missing",
                    key.key, index
                )));
            }
        }
    }
    Ok(observation_map)
}

fn decide_detection_key(
    detector: &EnvDetector,
    key: &EnvDetectionKey,
    context: &EnvironmentDetectionContext,
    observations: &BTreeMap<(String, usize), f32>,
) -> EnvironmentDecisionResult<EnvDetectedValue> {
    let mut best: Option<(&EnvDetectionCandidate, bool, f32)> = None;
    for (index, candidate) in key.candidates.iter().enumerate() {
        let confidence = *observations.get(&(key.key.clone(), index)).ok_or_else(|| {
            EnvironmentDecisionError::new(format!(
                "environment observation for key '{}' candidate {} is missing",
                key.key, index
            ))
        })?;
        let threshold = candidate.threshold.unwrap_or(key.min_confidence);
        let passed = confidence >= threshold;
        if best
            .as_ref()
            .is_none_or(|(_, _, best_score)| confidence > *best_score)
        {
            best = Some((candidate, passed, confidence));
        }
    }
    let Some((candidate, passed, confidence)) = best else {
        return Err(EnvironmentDecisionError::new(format!(
            "env key '{}' has no evaluated candidates",
            key.key
        )));
    };
    if !passed || confidence < key.min_confidence {
        return Err(EnvironmentDecisionError::new(format!(
            "env detector '{}' key '{}' needs detection: best candidate '{}' scored {:.6}, below threshold {:.6}",
            detector.id, key.key, candidate.value, confidence, key.min_confidence
        )));
    }
    validate_environment_value_safety(&candidate.value, &key.key)
        .map_err(|error| EnvironmentDecisionError::new(error.message()))?;
    if !key
        .allowed_values
        .iter()
        .any(|allowed| allowed == &candidate.value)
    {
        return Err(EnvironmentDecisionError::new(format!(
            "env key '{}' candidate value '{}' is not in allowed_values",
            key.key, candidate.value
        )));
    }
    Ok(EnvDetectedValue {
        value: candidate.value.clone(),
        confidence,
        source: candidate
            .source
            .clone()
            .unwrap_or_else(|| format!("{}@{}", detector.id, candidate.value)),
        detected_at_unix_ms: context.generated_at_unix_ms,
        detector_id: detector.id.clone(),
        expires_at_unix_ms: key
            .ttl_ms
            .map(|ttl| context.generated_at_unix_ms.saturating_add(ttl)),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentStateScope {
    pub instance_id: String,
    pub game_id: String,
    pub server_id: String,
    pub resource_pack_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnvironmentDetectorState {
    pub id: String,
    pub version: String,
    pub keys: Vec<EnvironmentKeyState>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnvironmentKeyState {
    pub key: String,
    pub stale_threshold: f32,
    pub allowed_values: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvDetectionResult {
    pub schema_version: String,
    pub instance_id: String,
    pub game_id: String,
    pub server_id: String,
    pub detector_id: String,
    pub detector_version: String,
    pub resource_pack_id: String,
    pub resource_pack_hash: String,
    pub generated_at_unix_ms: u64,
    pub detections: BTreeMap<String, EnvDetectedValue>,
}

impl EnvDetectionResult {
    pub fn detected_facts(&self) -> Vec<EnvDetected> {
        self.detections
            .iter()
            .map(|(key, value)| EnvDetected {
                key: key.clone(),
                value: value.value.clone(),
                confidence: value.confidence,
                source: value.source.clone(),
                detector_id: value.detector_id.clone(),
                detected_at_unix_ms: value.detected_at_unix_ms,
            })
            .collect()
    }

    pub fn resolved_facts(&self) -> Vec<EnvResolved> {
        self.detections
            .iter()
            .map(|(key, value)| EnvResolved {
                key: key.clone(),
                value: value.value.clone(),
                confidence: value.confidence,
                source: value.source.clone(),
                detector_id: value.detector_id.clone(),
                source_result: format!("{}@{}", self.detector_id, self.generated_at_unix_ms),
            })
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvDetectedValue {
    pub value: String,
    pub confidence: f32,
    pub source: String,
    pub detected_at_unix_ms: u64,
    pub detector_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at_unix_ms: Option<u64>,
}

/// Validates and resolves caller-supplied environment state without filesystem or device access.
pub struct EnvironmentStateEngine {
    scope: EnvironmentStateScope,
    detector: EnvironmentDetectorState,
}

impl EnvironmentStateEngine {
    pub fn new(scope: EnvironmentStateScope, detector: EnvironmentDetectorState) -> Self {
        Self { scope, detector }
    }

    pub fn validate_result(
        &self,
        result: &EnvDetectionResult,
        resource_hash: &str,
        now_ms: u64,
    ) -> EnvironmentStateResult<()> {
        if result.schema_version != ENV_RESULT_SCHEMA_VERSION {
            return Err(EnvironmentStateError::new(
                EnvironmentStateErrorKind::SchemaMismatch,
                format!(
                    "env detection result schema '{}' is stale; expected '{}'",
                    result.schema_version, ENV_RESULT_SCHEMA_VERSION
                ),
            ));
        }
        if result.instance_id != self.scope.instance_id {
            return Err(EnvironmentStateError::new(
                EnvironmentStateErrorKind::InstanceMismatch,
                "env detection result belongs to a different instance_id",
            ));
        }
        if result.game_id != self.scope.game_id || result.server_id != self.scope.server_id {
            return Err(EnvironmentStateError::new(
                EnvironmentStateErrorKind::ScopeMismatch,
                format!(
                    "env detection result scope is stale: result {}.{} command {}.{}",
                    result.game_id, result.server_id, self.scope.game_id, self.scope.server_id
                ),
            ));
        }
        if result.detector_id != self.detector.id
            || result.detector_version != self.detector.version
        {
            return Err(EnvironmentStateError::new(
                EnvironmentStateErrorKind::DetectorMismatch,
                format!(
                    "env detection result detector is stale: result {}@{} command {}@{}",
                    result.detector_id,
                    result.detector_version,
                    self.detector.id,
                    self.detector.version
                ),
            ));
        }
        if result.resource_pack_id != self.scope.resource_pack_id
            || result.resource_pack_hash != resource_hash
        {
            return Err(EnvironmentStateError::new(
                EnvironmentStateErrorKind::ResourceHashChanged,
                "env detection result is stale because detector resource hash changed",
            ));
        }
        for key in &self.detector.keys {
            let value = result.detections.get(&key.key).ok_or_else(|| {
                EnvironmentStateError::new(
                    EnvironmentStateErrorKind::MissingKey,
                    format!(
                        "env detection result is missing key '{}'; run detect first",
                        key.key
                    ),
                )
            })?;
            validate_resolved_value(&key.key, value, key, now_ms)?;
        }
        Ok(())
    }

    pub fn resolve_markers(
        &self,
        input: &str,
        result: &EnvDetectionResult,
        now_ms: u64,
    ) -> EnvironmentStateResult<(String, Vec<EnvResolved>)> {
        let mut output = String::new();
        let mut resolved = Vec::new();
        let mut offset = 0usize;
        while let Some(start_rel) = input[offset..].find("{env:") {
            let start = offset + start_rel;
            output.push_str(&input[offset..start]);
            let key_start = start + "{env:".len();
            let end_rel = input[key_start..].find('}').ok_or_else(|| {
                EnvironmentStateError::new(
                    EnvironmentStateErrorKind::InvalidPointer,
                    format!("malformed env pointer in '{input}': missing closing '}}'"),
                )
            })?;
            let end = key_start + end_rel;
            let key = &input[key_start..end];
            let value = self.resolve_key(key, result, now_ms)?;
            output.push_str(&value.value);
            resolved.push(value);
            offset = end + 1;
        }
        output.push_str(&input[offset..]);
        Ok((output, resolved))
    }

    pub fn resolve_value(
        &self,
        value: &mut Value,
        result: &EnvDetectionResult,
        now_ms: u64,
    ) -> EnvironmentStateResult<Vec<EnvResolved>> {
        let mut resolved = BTreeMap::new();
        self.resolve_value_inner(value, result, now_ms, &mut resolved)?;
        Ok(resolved.into_values().collect())
    }

    /// Resolves legacy `{env:}` markers from the Runtime-owned fact snapshot.
    pub fn resolve_markers_from_fact_snapshot(
        &self,
        input: &str,
        snapshot: &InstanceFactSnapshot,
        resource_hash: &str,
        now_ms: u64,
    ) -> EnvironmentStateResult<(String, Vec<EnvResolved>)> {
        self.validate_fact_snapshot_scope(snapshot)?;
        let mut output = String::new();
        let mut resolved = Vec::new();
        let mut offset = 0usize;
        while let Some(start_rel) = input[offset..].find("{env:") {
            let start = offset + start_rel;
            output.push_str(&input[offset..start]);
            let key_start = start + "{env:".len();
            let end_rel = input[key_start..].find('}').ok_or_else(|| {
                EnvironmentStateError::new(
                    EnvironmentStateErrorKind::InvalidPointer,
                    format!("malformed env pointer in '{input}': missing closing '}}'"),
                )
            })?;
            let end = key_start + end_rel;
            let key = &input[key_start..end];
            let value =
                self.resolve_key_from_fact_snapshot(key, snapshot, resource_hash, now_ms)?;
            output.push_str(&value.value);
            resolved.push(value);
            offset = end + 1;
        }
        output.push_str(&input[offset..]);
        Ok((output, resolved))
    }

    /// Resolves every legacy `{env:}` marker in a JSON value from one pinned fact snapshot.
    pub fn resolve_value_from_fact_snapshot(
        &self,
        value: &mut Value,
        snapshot: &InstanceFactSnapshot,
        resource_hash: &str,
        now_ms: u64,
    ) -> EnvironmentStateResult<Vec<EnvResolved>> {
        self.validate_fact_snapshot_scope(snapshot)?;
        let mut resolved = BTreeMap::new();
        self.resolve_fact_value_inner(value, snapshot, resource_hash, now_ms, &mut resolved)?;
        Ok(resolved.into_values().collect())
    }

    /// Resolves one declared environment key through the shared fact projection.
    pub fn resolve_key_from_fact_snapshot(
        &self,
        key: &str,
        snapshot: &InstanceFactSnapshot,
        resource_hash: &str,
        now_ms: u64,
    ) -> EnvironmentStateResult<EnvResolved> {
        self.validate_fact_snapshot_scope(snapshot)?;
        let key_config = self
            .detector
            .keys
            .iter()
            .find(|item| item.key == key)
            .ok_or_else(|| {
                EnvironmentStateError::new(
                    EnvironmentStateErrorKind::UndeclaredKey,
                    format!(
                        "env key '{key}' is not declared by detector '{}'",
                        self.detector.id
                    ),
                )
            })?;
        let fact_key = format!("env.{key}");
        let record = match snapshot.resolve(&fact_key, now_ms) {
            FactResolution::Known(record) => record,
            FactResolution::Unknown { reason, .. } => {
                return Err(EnvironmentStateError::new(
                    fact_unknown_kind(reason),
                    format!("env fact '{fact_key}' is unavailable: {reason:?}"),
                ));
            }
        };
        if record.source_detector != self.detector.id {
            return Err(EnvironmentStateError::new(
                EnvironmentStateErrorKind::DetectorMismatch,
                format!(
                    "env fact '{fact_key}' was produced by detector '{}' instead of '{}'",
                    record.source_detector, self.detector.id
                ),
            ));
        }
        if record.resource_bundle_hash != resource_hash {
            return Err(EnvironmentStateError::new(
                EnvironmentStateErrorKind::ResourceHashChanged,
                "env fact is stale because detector resource hash changed",
            ));
        }
        let FactContent::Inline {
            value: FactValue::String(value),
        } = &record.content
        else {
            return Err(EnvironmentStateError::new(
                EnvironmentStateErrorKind::SchemaMismatch,
                format!("env fact '{fact_key}' is not an inline string"),
            ));
        };
        validate_environment_value_safety(value, key)?;
        if !key_config
            .allowed_values
            .iter()
            .any(|allowed| allowed == value)
        {
            return Err(EnvironmentStateError::new(
                EnvironmentStateErrorKind::UnallowedValue,
                format!("env key '{key}' value '{value}' is not in allowed_values"),
            ));
        }
        let confidence = f32::from(record.confidence_milli) / 1_000.0;
        if confidence < key_config.stale_threshold {
            return Err(EnvironmentStateError::new(
                EnvironmentStateErrorKind::LowConfidence,
                format!(
                    "env key '{key}' is stale: confidence {confidence:.6} below threshold {:.6}",
                    key_config.stale_threshold
                ),
            ));
        }
        Ok(EnvResolved {
            key: key.to_owned(),
            value: value.clone(),
            confidence,
            source: record.source_snapshot_id.clone(),
            detector_id: record.source_detector.clone(),
            source_result: snapshot.snapshot_id.clone(),
        })
    }

    pub fn resolve_key(
        &self,
        key: &str,
        result: &EnvDetectionResult,
        now_ms: u64,
    ) -> EnvironmentStateResult<EnvResolved> {
        let key_config = self
            .detector
            .keys
            .iter()
            .find(|item| item.key == key)
            .ok_or_else(|| {
                EnvironmentStateError::new(
                    EnvironmentStateErrorKind::UndeclaredKey,
                    format!(
                        "env key '{key}' is not declared by detector '{}'",
                        self.detector.id
                    ),
                )
            })?;
        let value = result.detections.get(key).ok_or_else(|| {
            EnvironmentStateError::new(
                EnvironmentStateErrorKind::MissingKey,
                format!("env detection result is missing key '{key}'; run detect first"),
            )
        })?;
        validate_resolved_value(key, value, key_config, now_ms)?;
        Ok(EnvResolved {
            key: key.to_string(),
            value: value.value.clone(),
            confidence: value.confidence,
            source: value.source.clone(),
            detector_id: result.detector_id.clone(),
            source_result: format!("{}@{}", result.detector_id, result.generated_at_unix_ms),
        })
    }

    fn resolve_value_inner(
        &self,
        value: &mut Value,
        result: &EnvDetectionResult,
        now_ms: u64,
        resolved: &mut BTreeMap<String, EnvResolved>,
    ) -> EnvironmentStateResult<()> {
        match value {
            Value::String(text) => {
                let (replacement, keys) = self.resolve_markers(text, result, now_ms)?;
                *text = replacement;
                for key in keys {
                    resolved.entry(key.key.clone()).or_insert(key);
                }
            }
            Value::Array(values) => {
                for value in values {
                    self.resolve_value_inner(value, result, now_ms, resolved)?;
                }
            }
            Value::Object(object) => {
                for value in object.values_mut() {
                    self.resolve_value_inner(value, result, now_ms, resolved)?;
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) => {}
        }
        Ok(())
    }

    fn resolve_fact_value_inner(
        &self,
        value: &mut Value,
        snapshot: &InstanceFactSnapshot,
        resource_hash: &str,
        now_ms: u64,
        resolved: &mut BTreeMap<String, EnvResolved>,
    ) -> EnvironmentStateResult<()> {
        match value {
            Value::String(text) => {
                let (replacement, keys) =
                    self.resolve_markers_from_fact_snapshot(text, snapshot, resource_hash, now_ms)?;
                *text = replacement;
                for key in keys {
                    resolved.entry(key.key.clone()).or_insert(key);
                }
            }
            Value::Array(values) => {
                for value in values {
                    self.resolve_fact_value_inner(
                        value,
                        snapshot,
                        resource_hash,
                        now_ms,
                        resolved,
                    )?;
                }
            }
            Value::Object(object) => {
                for value in object.values_mut() {
                    self.resolve_fact_value_inner(
                        value,
                        snapshot,
                        resource_hash,
                        now_ms,
                        resolved,
                    )?;
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) => {}
        }
        Ok(())
    }

    fn validate_fact_snapshot_scope(
        &self,
        snapshot: &InstanceFactSnapshot,
    ) -> EnvironmentStateResult<()> {
        snapshot.validate().map_err(|_| {
            EnvironmentStateError::new(
                EnvironmentStateErrorKind::SchemaMismatch,
                "instance fact snapshot is invalid",
            )
        })?;
        if snapshot.context.instance_id != self.scope.instance_id {
            return Err(EnvironmentStateError::new(
                EnvironmentStateErrorKind::InstanceMismatch,
                "instance fact snapshot belongs to a different instance_id",
            ));
        }
        if snapshot.context.game_id != self.scope.game_id
            || snapshot.context.server_id != self.scope.server_id
        {
            return Err(EnvironmentStateError::new(
                EnvironmentStateErrorKind::ScopeMismatch,
                "instance fact snapshot belongs to a different game or server",
            ));
        }
        Ok(())
    }
}

const fn fact_unknown_kind(reason: FactUnknownReason) -> EnvironmentStateErrorKind {
    match reason {
        FactUnknownReason::Missing => EnvironmentStateErrorKind::MissingKey,
        FactUnknownReason::Expired => EnvironmentStateErrorKind::Expired,
        FactUnknownReason::LowConfidence => EnvironmentStateErrorKind::LowConfidence,
        FactUnknownReason::NonInline | FactUnknownReason::TypeMismatch => {
            EnvironmentStateErrorKind::SchemaMismatch
        }
    }
}

pub fn collect_environment_pointer_keys(value: &Value) -> EnvironmentStateResult<BTreeSet<String>> {
    let mut keys = BTreeSet::new();
    collect_environment_pointer_keys_inner(value, &mut keys)?;
    Ok(keys)
}

pub fn validate_environment_value_safety(value: &str, key: &str) -> EnvironmentStateResult<()> {
    if value.is_empty()
        || value == "."
        || value.contains('/')
        || value.contains('\\')
        || value.contains(':')
        || value.contains("..")
        || Path::new(value).is_absolute()
    {
        return Err(EnvironmentStateError::new(
            EnvironmentStateErrorKind::UnsafeValue,
            format!("env key '{key}' has unsafe value '{value}'"),
        ));
    }
    Ok(())
}

fn validate_resolved_value(
    key: &str,
    value: &EnvDetectedValue,
    key_config: &EnvironmentKeyState,
    now_ms: u64,
) -> EnvironmentStateResult<()> {
    validate_environment_value_safety(&value.value, key)?;
    if !key_config
        .allowed_values
        .iter()
        .any(|allowed| allowed == &value.value)
    {
        return Err(EnvironmentStateError::new(
            EnvironmentStateErrorKind::UnallowedValue,
            format!(
                "env key '{key}' value '{}' is not in allowed_values",
                value.value
            ),
        ));
    }
    if value.confidence < key_config.stale_threshold {
        return Err(EnvironmentStateError::new(
            EnvironmentStateErrorKind::LowConfidence,
            format!(
                "env key '{key}' is stale: confidence {:.6} below threshold {:.6}",
                value.confidence, key_config.stale_threshold
            ),
        ));
    }
    if let Some(expires_at) = value.expires_at_unix_ms
        && now_ms > expires_at
    {
        return Err(EnvironmentStateError::new(
            EnvironmentStateErrorKind::Expired,
            format!("env key '{key}' expired at {expires_at}; run detect first"),
        ));
    }
    Ok(())
}

fn collect_environment_pointer_keys_inner(
    value: &Value,
    keys: &mut BTreeSet<String>,
) -> EnvironmentStateResult<()> {
    match value {
        Value::String(text) => collect_environment_pointer_keys_from_str(text, keys)?,
        Value::Array(values) => {
            for value in values {
                collect_environment_pointer_keys_inner(value, keys)?;
            }
        }
        Value::Object(object) => {
            for value in object.values() {
                collect_environment_pointer_keys_inner(value, keys)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
    Ok(())
}

fn collect_environment_pointer_keys_from_str(
    text: &str,
    keys: &mut BTreeSet<String>,
) -> EnvironmentStateResult<()> {
    let mut offset = 0usize;
    while let Some(start_rel) = text[offset..].find("{env:") {
        let key_start = offset + start_rel + "{env:".len();
        let end_rel = text[key_start..].find('}').ok_or_else(|| {
            EnvironmentStateError::new(
                EnvironmentStateErrorKind::InvalidPointer,
                format!("malformed env pointer in '{text}': missing closing '}}'"),
            )
        })?;
        let end = key_start + end_rel;
        let key = &text[key_start..end];
        if key.trim().is_empty() {
            return Err(EnvironmentStateError::new(
                EnvironmentStateErrorKind::InvalidPointer,
                "env pointer key must not be empty",
            ));
        }
        keys.insert(key.to_string());
        offset = end + 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_contract::{
        EventType, FactRecord, FactScope, FactTtlPolicy, FactTtlSource, InstanceFactContext,
    };
    use serde_json::json;

    #[test]
    fn valid_result_resolves_nested_markers() {
        let engine = engine();
        let result = result("Default", 0.95, None);
        engine
            .validate_result(&result, "hash", 100)
            .expect("fresh result");
        let mut value = json!({
            "path": "hometheme/{env:ui_theme}/Depot.png",
            "nested": ["{env:ui_theme}"]
        });
        let resolved = engine
            .resolve_value(&mut value, &result, 100)
            .expect("resolve value");
        assert_eq!(value["path"], "hometheme/Default/Depot.png");
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].value, "Default");
    }

    #[test]
    fn fact_snapshot_drives_the_existing_environment_pointer_surface() {
        let snapshot = fact_snapshot("Default", 950, Some(200));
        let mut value = json!({
            "path": "hometheme/{env:ui_theme}/Depot.png",
            "nested": ["{env:ui_theme}"]
        });
        let resolved = engine()
            .resolve_value_from_fact_snapshot(&mut value, &snapshot, &"a".repeat(64), 100)
            .expect("resolve fact-backed value");
        assert_eq!(value["path"], "hometheme/Default/Depot.png");
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].source_result, snapshot.snapshot_id);

        let error = engine()
            .resolve_key_from_fact_snapshot("ui_theme", &snapshot, &"a".repeat(64), 201)
            .expect_err("expired fact must request fresh detection");
        assert_eq!(error.kind(), EnvironmentStateErrorKind::Expired);
    }

    #[test]
    fn freshness_failures_have_typed_reasons() {
        let mut stale = result("Default", 0.95, None);
        stale.resource_pack_hash = "old".to_string();
        let error = engine()
            .validate_result(&stale, "hash", 100)
            .expect_err("resource change must fail");
        assert_eq!(error.kind(), EnvironmentStateErrorKind::ResourceHashChanged);
        assert_eq!(error.kind().reason(), "resource_hash_changed");

        let low = result("Default", 0.4, None);
        let error = engine()
            .validate_result(&low, "hash", 100)
            .expect_err("low confidence must fail");
        assert_eq!(error.kind(), EnvironmentStateErrorKind::LowConfidence);
    }

    #[test]
    fn unsafe_unlisted_and_expired_values_fail_visibly() {
        let cases = [
            (
                result("../Default", 0.95, None),
                EnvironmentStateErrorKind::UnsafeValue,
            ),
            (
                result("Other", 0.95, None),
                EnvironmentStateErrorKind::UnallowedValue,
            ),
            (
                result("Default", 0.95, Some(99)),
                EnvironmentStateErrorKind::Expired,
            ),
        ];
        for (result, expected) in cases {
            let error = engine()
                .validate_result(&result, "hash", 100)
                .expect_err("invalid state must fail");
            assert_eq!(error.kind(), expected);
        }
    }

    #[test]
    fn pointer_collection_rejects_malformed_or_empty_keys() {
        let missing_close = collect_environment_pointer_keys(&json!("{env:ui_theme"))
            .expect_err("missing close must fail");
        assert_eq!(
            missing_close.kind(),
            EnvironmentStateErrorKind::InvalidPointer
        );

        let empty =
            collect_environment_pointer_keys(&json!("{env:}")).expect_err("empty key must fail");
        assert_eq!(empty.kind(), EnvironmentStateErrorKind::InvalidPointer);
    }

    #[test]
    fn click_free_state_engine_has_no_undeclared_key_fallback() {
        let error = engine()
            .resolve_key("missing", &result("Default", 0.95, None), 100)
            .expect_err("undeclared key must fail");
        assert_eq!(error.kind(), EnvironmentStateErrorKind::UndeclaredKey);
    }

    #[test]
    fn environment_selectors_are_generic_and_strict() {
        assert_eq!(
            canonical_environment_game(" Fixture-Game ").expect("selector"),
            "fixture-game"
        );
        assert_eq!(default_environment_server("fixture-game"), "default");
        assert!(canonical_environment_game("fixture/game").is_err());
        assert!(canonical_environment_game(" ").is_err());
    }

    #[test]
    fn flat_catalog_is_normalized_and_validated() {
        let catalog = parse_environment_catalog_value(json!({
            "schema_version": "env-detections.v1",
            "game": "fixture-game-a",
            "detections": [{
                "detector_id": "detect_ui_theme",
                "detector_version": "1",
                "key": "ui_theme",
                "threshold": 0.7,
                "invalidate_below_confidence": 0.6,
                "allowed_values": ["Default"],
                "candidates": [{
                    "value": "Default",
                    "template": "hometheme/Default/Terminal.png",
                    "roi": [844, 58, 268, 272]
                }]
            }]
        }))
        .expect("parse flat catalog");
        catalog.validate().expect("validate flat catalog");

        let detector = catalog.detector("detect_ui_theme").expect("detector");
        assert_eq!(detector.game_id.as_deref(), Some("fixture-game-a"));
        assert_eq!(detector.keys[0].stale_threshold(), 0.6);
        assert_eq!(
            detector.keys[0].candidates[0].region,
            Some(EnvRect {
                x: 844,
                y: 58,
                width: 268,
                height: 272,
            })
        );
    }

    #[test]
    fn catalog_validation_rejects_invalid_candidates_and_duplicate_keys() {
        let mut mixed = catalog("detect_a", "ui_theme");
        mixed.detections[0].keys[0].candidates[0].width = Some(1280);
        mixed.detections[0].keys[0].candidates[0].height = Some(720);
        let error = mixed.validate().expect_err("mixed matcher must fail");
        assert!(
            error
                .message()
                .contains("must not mix template and scene size")
        );

        let mut duplicate = catalog("detect_a", "ui_theme");
        let duplicate_key = duplicate.detections[0].keys[0].clone();
        duplicate.detections[0].keys.push(duplicate_key);
        let error = duplicate.validate().expect_err("duplicate key must fail");
        assert!(error.message().contains("key 'ui_theme' is duplicated"));
    }

    #[test]
    fn detector_selection_requires_one_unambiguous_owner() {
        let mut catalog_value = catalog("detect_a", "ui_theme");
        let second = catalog("detect_b", "ui_theme").detections.remove(0);
        catalog_value.detections.push(second);
        let keys = BTreeSet::from(["ui_theme".to_string()]);
        let ambiguous = catalog_value
            .select_detector_for_keys(None, &keys)
            .expect_err("ambiguous keys must fail");
        assert!(ambiguous.message().contains("ambiguous across detectors"));
        assert_eq!(
            catalog_value
                .select_detector_for_keys(Some("detect_b"), &keys)
                .expect("explicit detector")
                .id,
            "detect_b"
        );
    }

    #[test]
    fn detection_steps_are_normalized_without_device_authority() {
        let step = EnvDetectionStep {
            kind: "long-tap".to_string(),
            x: Some(10),
            y: Some(20),
            x1: None,
            y1: None,
            x2: None,
            y2: None,
            duration_ms: Some(500),
        };
        let plan = serde_json::to_value(step.plan().expect("step plan")).expect("serialize plan");
        assert_eq!(plan["type"], "long_tap");

        let invalid = EnvDetectionStep { y: None, ..step };
        assert!(
            invalid
                .plan()
                .expect_err("missing coordinate must fail")
                .message()
                .contains("missing coordinate y")
        );
    }

    #[test]
    fn candidate_observations_select_best_and_construct_result() {
        let mut detector = catalog("detect_a", "ui_theme").detections.remove(0);
        detector.keys[0].allowed_values.push("Other".to_string());
        detector.keys[0].ttl_ms = Some(50);
        detector.keys[0].candidates.push(EnvDetectionCandidate {
            value: "Other".to_string(),
            template_path: Some("other.png".to_string()),
            width: None,
            height: None,
            region: None,
            threshold: None,
            source: Some("fixture-other".to_string()),
        });

        let result = EnvironmentDetectionEngine::decide(
            &detector,
            &decision_context(),
            vec![observation(0, 0.8), observation(1, 0.9)],
        )
        .expect("decision");
        let value = &result.detections["ui_theme"];
        assert_eq!(value.value, "Other");
        assert_eq!(value.source, "fixture-other");
        assert_eq!(value.expires_at_unix_ms, Some(150));
        assert_eq!(result.resource_pack_hash, "hash");
    }

    #[test]
    fn candidate_decision_requires_complete_unique_observations() {
        let detector = catalog("detect_a", "ui_theme").detections.remove(0);
        let missing =
            EnvironmentDetectionEngine::decide(&detector, &decision_context(), Vec::new())
                .expect_err("missing observation must fail");
        assert!(missing.message().contains("candidate 0 is missing"));

        let duplicate = EnvironmentDetectionEngine::decide(
            &detector,
            &decision_context(),
            vec![observation(0, 0.8), observation(0, 0.9)],
        )
        .expect_err("duplicate observation must fail");
        assert!(duplicate.message().contains("candidate 0 is duplicated"));
    }

    #[test]
    fn below_threshold_or_invalid_confidence_never_becomes_success() {
        let detector = catalog("detect_a", "ui_theme").detections.remove(0);
        let below = EnvironmentDetectionEngine::decide(
            &detector,
            &decision_context(),
            vec![observation(0, 0.6)],
        )
        .expect_err("below threshold must fail");
        assert!(below.message().contains("below threshold 0.700000"));

        let invalid = EnvironmentDetectionEngine::decide(
            &detector,
            &decision_context(),
            vec![observation(0, f32::NAN)],
        )
        .expect_err("invalid confidence must fail");
        assert!(invalid.message().contains("invalid confidence"));
    }

    fn engine() -> EnvironmentStateEngine {
        EnvironmentStateEngine::new(
            EnvironmentStateScope {
                instance_id: "envinst_a".to_string(),
                game_id: "fixture-game-a".to_string(),
                server_id: "region-a".to_string(),
                resource_pack_id: "test-pack".to_string(),
            },
            EnvironmentDetectorState {
                id: "detect_ui_theme".to_string(),
                version: "1".to_string(),
                keys: vec![EnvironmentKeyState {
                    key: "ui_theme".to_string(),
                    stale_threshold: 0.7,
                    allowed_values: vec!["Default".to_string()],
                }],
            },
        )
    }

    fn catalog(detector_id: &str, key: &str) -> EnvDetectionCatalog {
        EnvDetectionCatalog {
            schema_version: Some("env-detection.v1".to_string()),
            detections: vec![EnvDetector {
                id: detector_id.to_string(),
                version: Some("1".to_string()),
                game_id: Some("fixture-game-a".to_string()),
                server_id: Some("region-a".to_string()),
                resource_pack_id: Some("test-pack".to_string()),
                match_metric: Some("ccorr_normed".to_string()),
                steps: Vec::new(),
                keys: vec![EnvDetectionKey {
                    key: key.to_string(),
                    min_confidence: 0.7,
                    stale_below_confidence: Some(0.6),
                    ttl_ms: None,
                    allowed_values: vec!["Default".to_string()],
                    candidates: vec![EnvDetectionCandidate {
                        value: "Default".to_string(),
                        template_path: Some("template.png".to_string()),
                        width: None,
                        height: None,
                        region: None,
                        threshold: None,
                        source: None,
                    }],
                }],
            }],
        }
    }

    fn decision_context() -> EnvironmentDetectionContext {
        EnvironmentDetectionContext {
            instance_id: "envinst_a".to_string(),
            game_id: "fixture-game-a".to_string(),
            server_id: "region-a".to_string(),
            resource_pack_hash: "hash".to_string(),
            generated_at_unix_ms: 100,
        }
    }

    fn observation(candidate_index: usize, confidence: f32) -> EnvironmentCandidateObservation {
        EnvironmentCandidateObservation {
            key: "ui_theme".to_string(),
            candidate_index,
            confidence,
        }
    }

    fn result(value: &str, confidence: f32, expires_at_unix_ms: Option<u64>) -> EnvDetectionResult {
        EnvDetectionResult {
            schema_version: ENV_RESULT_SCHEMA_VERSION.to_string(),
            instance_id: "envinst_a".to_string(),
            game_id: "fixture-game-a".to_string(),
            server_id: "region-a".to_string(),
            detector_id: "detect_ui_theme".to_string(),
            detector_version: "1".to_string(),
            resource_pack_id: "test-pack".to_string(),
            resource_pack_hash: "hash".to_string(),
            generated_at_unix_ms: 50,
            detections: BTreeMap::from([(
                "ui_theme".to_string(),
                EnvDetectedValue {
                    value: value.to_string(),
                    confidence,
                    source: "fixture".to_string(),
                    detected_at_unix_ms: 50,
                    detector_id: "detect_ui_theme".to_string(),
                    expires_at_unix_ms,
                },
            )]),
        }
    }

    fn fact_snapshot(
        value: &str,
        confidence_milli: u16,
        expires_at_unix_ms: Option<u64>,
    ) -> InstanceFactSnapshot {
        InstanceFactSnapshot {
            snapshot_id: "snapshot:fact".to_string(),
            ledger_position: 1,
            context: InstanceFactContext {
                instance_id: "envinst_a".to_string(),
                game_id: "arknights".to_string(),
                server_id: "cn".to_string(),
            },
            records: vec![FactRecord {
                scope: FactScope::Server {
                    server_id: "cn".to_string(),
                },
                key: "env.ui_theme".to_string(),
                content: FactContent::Inline {
                    value: FactValue::String(value.to_string()),
                },
                observed_at_unix_ms: 50,
                expires_at_unix_ms,
                ttl_policy: expires_at_unix_ms.map(|expires| FactTtlPolicy {
                    minimum_ms: 1,
                    maximum_ms: expires - 50,
                    source: FactTtlSource::DetectorContract,
                }),
                confidence_milli,
                source_detector: "detect_ui_theme".to_string(),
                source_snapshot_id: "snapshot:detection".to_string(),
                schema_version: "fact.v1".to_string(),
                resource_bundle_hash: "a".repeat(64),
                invalidate_on: vec![EventType::RuntimeTakeover],
            }],
        }
    }
}
