// SPDX-License-Identifier: AGPL-3.0-only

//! Pure environment-result validation and marker-resolution decisions.

use actingcommand_contract::{EnvDetected, EnvResolved};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::path::Path;

pub const ENV_RESULT_SCHEMA_VERSION: &str = "env-detect-result.v1";

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

    fn engine() -> EnvironmentStateEngine {
        EnvironmentStateEngine::new(
            EnvironmentStateScope {
                instance_id: "envinst_a".to_string(),
                game_id: "arknights".to_string(),
                server_id: "cn".to_string(),
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

    fn result(value: &str, confidence: f32, expires_at_unix_ms: Option<u64>) -> EnvDetectionResult {
        EnvDetectionResult {
            schema_version: ENV_RESULT_SCHEMA_VERSION.to_string(),
            instance_id: "envinst_a".to_string(),
            game_id: "arknights".to_string(),
            server_id: "cn".to_string(),
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
}
