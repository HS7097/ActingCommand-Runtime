// SPDX-License-Identifier: AGPL-3.0-only

use super::{ArtifactId, CorrelationId, FrameId, RunId, SanitizationError, StaticCode};
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionClass {
    DebugFull,
    Adaptive,
    Light,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactRedactionState {
    NotRequired,
    Applied,
    Pending,
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct ArtifactReference {
    artifact_id: ArtifactId,
    kind: StaticCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    run_id: Option<RunId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    frame_id: Option<FrameId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    correlation_id: Option<CorrelationId>,
    object_key: String,
    media_type: String,
    byte_count: u64,
    sha256: String,
    created_at_unix_ms: u64,
    producer: StaticCode,
    retention_class: RetentionClass,
    redaction_state: ArtifactRedactionState,
}

impl ArtifactReference {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        artifact_id: ArtifactId,
        kind: StaticCode,
        run_id: Option<RunId>,
        frame_id: Option<FrameId>,
        correlation_id: Option<CorrelationId>,
        object_key: impl Into<String>,
        media_type: impl Into<String>,
        byte_count: u64,
        sha256: impl Into<String>,
        created_at_unix_ms: u64,
        producer: StaticCode,
        retention_class: RetentionClass,
        redaction_state: ArtifactRedactionState,
    ) -> Result<Self, SanitizationError> {
        let reference = Self {
            artifact_id,
            kind,
            run_id,
            frame_id,
            correlation_id,
            object_key: object_key.into(),
            media_type: media_type.into(),
            byte_count,
            sha256: sha256.into(),
            created_at_unix_ms,
            producer,
            retention_class,
            redaction_state,
        };
        reference.validate()?;
        Ok(reference)
    }

    pub fn artifact_id(&self) -> &ArtifactId {
        &self.artifact_id
    }

    pub fn kind(&self) -> &StaticCode {
        &self.kind
    }

    pub fn run_id(&self) -> Option<&RunId> {
        self.run_id.as_ref()
    }

    pub fn frame_id(&self) -> Option<&FrameId> {
        self.frame_id.as_ref()
    }

    pub fn correlation_id(&self) -> Option<&CorrelationId> {
        self.correlation_id.as_ref()
    }

    pub fn object_key(&self) -> &str {
        &self.object_key
    }

    pub fn media_type(&self) -> &str {
        &self.media_type
    }

    pub fn byte_count(&self) -> u64 {
        self.byte_count
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    pub fn created_at_unix_ms(&self) -> u64 {
        self.created_at_unix_ms
    }

    pub fn producer(&self) -> &StaticCode {
        &self.producer
    }

    pub fn retention_class(&self) -> RetentionClass {
        self.retention_class
    }

    pub fn redaction_state(&self) -> ArtifactRedactionState {
        self.redaction_state
    }

    pub fn validate(&self) -> Result<(), SanitizationError> {
        if !is_object_key(&self.object_key) {
            return Err(SanitizationError::new(
                "invalid_artifact_object_key",
                "object_key",
            ));
        }
        if !is_media_type(&self.media_type) {
            return Err(SanitizationError::new(
                "invalid_artifact_media_type",
                "media_type",
            ));
        }
        if self.byte_count == 0 {
            return Err(SanitizationError::new(
                "invalid_artifact_byte_count",
                "byte_count",
            ));
        }
        if !is_sha256(&self.sha256) {
            return Err(SanitizationError::new("invalid_artifact_hash", "sha256"));
        }
        if self.created_at_unix_ms == 0 {
            return Err(SanitizationError::new(
                "invalid_artifact_timestamp",
                "created_at_unix_ms",
            ));
        }
        Ok(())
    }

    pub fn project(&self, include_object_key: bool) -> ProjectedArtifactReference {
        ProjectedArtifactReference {
            artifact_id: self.artifact_id,
            kind: self.kind.clone(),
            run_id: self.run_id,
            frame_id: self.frame_id,
            correlation_id: self.correlation_id,
            object_key: include_object_key.then(|| self.object_key.clone()),
            media_type: self.media_type.clone(),
            byte_count: self.byte_count,
            sha256: self.sha256.clone(),
            created_at_unix_ms: self.created_at_unix_ms,
            producer: self.producer.clone(),
            retention_class: self.retention_class,
            redaction_state: self.redaction_state,
        }
    }
}

impl fmt::Debug for ArtifactReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ArtifactReference")
            .field("artifact_id", &self.artifact_id)
            .field("kind", &self.kind)
            .field("object_key", &"<redacted-object-key>")
            .field("byte_count", &self.byte_count)
            .field("retention_class", &self.retention_class)
            .field("redaction_state", &self.redaction_state)
            .finish()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArtifactReferenceRecord {
    artifact_id: ArtifactId,
    kind: StaticCode,
    run_id: Option<RunId>,
    frame_id: Option<FrameId>,
    correlation_id: Option<CorrelationId>,
    object_key: String,
    media_type: String,
    byte_count: u64,
    sha256: String,
    created_at_unix_ms: u64,
    producer: StaticCode,
    retention_class: RetentionClass,
    redaction_state: ArtifactRedactionState,
}

impl<'de> Deserialize<'de> for ArtifactReference {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let record = ArtifactReferenceRecord::deserialize(deserializer)?;
        Self::new(
            record.artifact_id,
            record.kind,
            record.run_id,
            record.frame_id,
            record.correlation_id,
            record.object_key,
            record.media_type,
            record.byte_count,
            record.sha256,
            record.created_at_unix_ms,
            record.producer,
            record.retention_class,
            record.redaction_state,
        )
        .map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectedArtifactReference {
    pub artifact_id: ArtifactId,
    pub kind: StaticCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame_id: Option<FrameId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<CorrelationId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object_key: Option<String>,
    pub media_type: String,
    pub byte_count: u64,
    pub sha256: String,
    pub created_at_unix_ms: u64,
    pub producer: StaticCode,
    pub retention_class: RetentionClass,
    pub redaction_state: ArtifactRedactionState,
}

fn is_object_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value.is_ascii()
        && !value.starts_with('/')
        && !value.starts_with('\\')
        && !value.contains(':')
        && !value.contains('\\')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/'))
        && value
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

fn is_media_type(value: &str) -> bool {
    let Some((kind, subtype)) = value.split_once('/') else {
        return false;
    };
    !kind.is_empty()
        && !subtype.is_empty()
        && !subtype.contains('/')
        && value.len() <= 128
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#' | b'$' | b'&' | b'-' | b'.' | b'^' | b'_' | b'+' | b'/'
                )
        })
}

fn is_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}
