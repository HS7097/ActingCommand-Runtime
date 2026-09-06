// SPDX-License-Identifier: AGPL-3.0-only

use super::{
    ArtifactId, CorrelationId, FrameId, IssuedCorrelationId, IssuedFrameId, IssuedRunId, RunId,
    SanitizationError, Sensitivity,
};
use super::{IdentifierIssuanceError, IdentifierIssuer};
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::io::Read;

pub const EFFECTIVE_CONFIGURATION_SCHEMA: &str =
    "actingcommand.runtime.effective-task-configuration.v1";
pub const MAX_EFFECTIVE_CONFIGURATION_BYTES: u64 = 1_048_576;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectiveDeviceConfiguration {
    pub input_backend: String,
    pub capture_backend: String,
    pub input_adb: String,
    pub capture_adb: String,
    pub configured_serial: Option<String>,
    pub resolved_serial: String,
    pub input_command_timeout_ms: u64,
    pub capture_command_timeout_ms: u64,
    pub capture_timeout_ms: u64,
    pub configured_mumu_root: Option<std::path::PathBuf>,
    pub configured_capture_dll: Option<std::path::PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectiveTimingSource {
    Control,
    Default,
    ExpectAfter,
    Operation,
    NotSpecified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectiveTimingValue {
    pub milliseconds: u64,
    pub source: EffectiveTimingSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectiveOperationTiming {
    pub operation_id: String,
    pub expect_after: bool,
    pub timeout: EffectiveTimingValue,
    pub interval: EffectiveTimingValue,
    pub postdelay: EffectiveTimingValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectiveTaskTiming {
    pub task_timeout_ms: Option<u64>,
    pub control_timeout_ms: Option<u64>,
    pub task_timeout: EffectiveTimingValue,
    pub step_timeout: EffectiveTimingValue,
    pub capture_interval: EffectiveTimingValue,
    pub operations: Vec<EffectiveOperationTiming>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectiveCaptureSelection {
    pub requested_backend: String,
    pub configured_adb: String,
    pub configured_serial: Option<String>,
    pub resolved_adb: String,
    pub selected_serial: String,
    pub mumu: Option<EffectiveMumuInstallation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectiveMumuInstallation {
    pub root: std::path::PathBuf,
    pub adb_path: std::path::PathBuf,
    pub capture_dll_path: std::path::PathBuf,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectiveInputSelection {
    pub backend: String,
    pub serial: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
pub enum EffectiveConfigurationFacts {
    Initial {
        device: Option<EffectiveDeviceConfiguration>,
        timing: EffectiveTaskTiming,
        request_timeout_ms: u64,
        host_deadline_monotonic_ms: u64,
        observed_at_monotonic_ms: u64,
        host_remaining_ms: u64,
        capture_observed: bool,
        input_observed: bool,
    },
    EntryRecovery {
        package_sha256: String,
        timing: EffectiveTaskTiming,
    },
    Capture {
        backend: String,
        selection: Option<EffectiveCaptureSelection>,
    },
    Input {
        selection: Option<EffectiveInputSelection>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectiveConfigurationRecord {
    pub schema_version: String,
    pub request_id: super::RequestId,
    pub task_id: super::TaskId,
    pub run_id: RunId,
    pub frame_id: Option<FrameId>,
    pub action_id: Option<super::ActionId>,
    pub source_sequence: Option<u64>,
    pub facts: EffectiveConfigurationFacts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ArtifactKind {
    #[serde(rename = "capture.frame")]
    CaptureFrame,
    #[serde(rename = "diagnostic.json")]
    DiagnosticJson,
    #[serde(rename = "evidence.archive")]
    EvidenceArchive,
    #[serde(rename = "evidence.manifest")]
    EvidenceManifest,
    #[serde(rename = "report.text")]
    TextReport,
    #[serde(rename = "report.strategy")]
    StrategyReport,
}

impl ArtifactKind {
    pub const fn media_type(self) -> ArtifactMediaType {
        match self {
            Self::CaptureFrame => ArtifactMediaType::ImagePng,
            Self::DiagnosticJson | Self::EvidenceManifest | Self::StrategyReport => {
                ArtifactMediaType::ApplicationJson
            }
            Self::EvidenceArchive => ArtifactMediaType::ApplicationZip,
            Self::TextReport => ArtifactMediaType::TextPlain,
        }
    }

    pub const fn default_retention_class(self) -> RetentionClass {
        match self {
            Self::CaptureFrame
            | Self::DiagnosticJson
            | Self::EvidenceArchive
            | Self::EvidenceManifest
            | Self::TextReport
            | Self::StrategyReport => RetentionClass::Adaptive,
        }
    }

    pub const fn extension(self) -> &'static str {
        match self {
            Self::CaptureFrame => "png",
            Self::DiagnosticJson | Self::EvidenceManifest | Self::StrategyReport => "json",
            Self::EvidenceArchive => "zip",
            Self::TextReport => "txt",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ArtifactMediaType {
    #[serde(rename = "image/png")]
    ImagePng,
    #[serde(rename = "application/json")]
    ApplicationJson,
    #[serde(rename = "application/zip")]
    ApplicationZip,
    #[serde(rename = "text/plain")]
    TextPlain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactProducer {
    CaptureStore,
    CapturePipeline,
    ArtifactStore,
    EvidenceExporter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionClass {
    DebugFull,
    Adaptive,
    Light,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactRedactionState {
    NotRequired,
    Applied,
    Pending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArtifactIssuePolicy {
    producer: ArtifactProducer,
    retention_class: RetentionClass,
    redaction_state: ArtifactRedactionState,
}

impl ArtifactIssuePolicy {
    pub const fn new(
        producer: ArtifactProducer,
        retention_class: RetentionClass,
        redaction_state: ArtifactRedactionState,
    ) -> Self {
        Self {
            producer,
            retention_class,
            redaction_state,
        }
    }
}

macro_rules! non_disclosing_enum_deserialize {
    ($name:ident { $($wire:literal => $variant:ident),+ $(,)? }) => {
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                struct EnumVisitor;

                impl Visitor<'_> for EnumVisitor {
                    type Value = $name;

                    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                        formatter.write_str("a schema-owned artifact value")
                    }

                    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
                    where
                        E: de::Error,
                    {
                        match value {
                            $($wire => Ok($name::$variant)),+,
                            _ => Err(E::custom("invalid schema-owned artifact value")),
                        }
                    }
                }

                deserializer.deserialize_str(EnumVisitor)
            }
        }
    };
}

non_disclosing_enum_deserialize!(ArtifactKind {
    "capture.frame" => CaptureFrame,
    "diagnostic.json" => DiagnosticJson,
    "evidence.archive" => EvidenceArchive,
    "evidence.manifest" => EvidenceManifest,
    "report.text" => TextReport,
    "report.strategy" => StrategyReport,
});
non_disclosing_enum_deserialize!(ArtifactMediaType {
    "image/png" => ImagePng,
    "application/json" => ApplicationJson,
    "application/zip" => ApplicationZip,
    "text/plain" => TextPlain,
});
non_disclosing_enum_deserialize!(ArtifactProducer {
    "capture_store" => CaptureStore,
    "capture_pipeline" => CapturePipeline,
    "artifact_store" => ArtifactStore,
    "evidence_exporter" => EvidenceExporter,
});
non_disclosing_enum_deserialize!(RetentionClass {
    "debug_full" => DebugFull,
    "adaptive" => Adaptive,
    "light" => Light,
});
non_disclosing_enum_deserialize!(ArtifactRedactionState {
    "not_required" => NotRequired,
    "applied" => Applied,
    "pending" => Pending,
});

/// Store-facing correlations. Transport IDs cannot be promoted into these slots.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArtifactLinksDraft {
    run_id: Option<IssuedRunId>,
    frame_id: Option<IssuedFrameId>,
    correlation_id: Option<IssuedCorrelationId>,
}

impl ArtifactLinksDraft {
    pub fn with_run_id(mut self, value: IssuedRunId) -> Self {
        self.run_id = Some(value);
        self
    }

    pub fn with_frame_id(mut self, value: IssuedFrameId) -> Self {
        self.frame_id = Some(value);
        self
    }

    pub fn with_correlation_id(mut self, value: IssuedCorrelationId) -> Self {
        self.correlation_id = Some(value);
        self
    }
}

/// Opaque material calculated from actual bytes, without an identity or publication authority.
pub struct ArtifactMaterial {
    byte_count: u64,
    sha256: String,
}

impl ArtifactMaterial {
    /// Consumes a reader to EOF using a fixed 64 KiB buffer. Declared hashes are not inputs.
    pub fn read_from(reader: &mut impl Read) -> std::io::Result<Self> {
        let mut material = ArtifactMaterialAccumulator::default();
        let mut buffer = [0_u8; 65_536];
        loop {
            let count = reader.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            material.update(&buffer[..count])?;
        }
        Ok(material.finish())
    }

    pub const fn byte_count(&self) -> u64 {
        self.byte_count
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }
}

/// Bounded calculation state. Only bytes can contribute to the resulting material.
#[derive(Default)]
pub struct ArtifactMaterialAccumulator {
    byte_count: u64,
    hasher: Sha256,
}

impl ArtifactMaterialAccumulator {
    pub fn update(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.byte_count = self
            .byte_count
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| std::io::Error::other("artifact byte count exceeds u64"))?;
        self.hasher.update(bytes);
        Ok(())
    }

    pub const fn byte_count(&self) -> u64 {
        self.byte_count
    }

    pub fn finish(self) -> ArtifactMaterial {
        ArtifactMaterial {
            byte_count: self.byte_count,
            sha256: format!("sha256:{:x}", self.hasher.finalize()),
        }
    }
}

/// Actual bytes or previously calculated opaque material for the store's sole issuance entry.
pub enum ArtifactIssueInput<'a> {
    Bytes(&'a [u8]),
    Material(ArtifactMaterial),
}

impl<'a, T: AsRef<[u8]> + ?Sized> From<&'a T> for ArtifactIssueInput<'a> {
    fn from(bytes: &'a T) -> Self {
        Self::Bytes(bytes.as_ref())
    }
}

impl From<ArtifactMaterial> for ArtifactIssueInput<'_> {
    fn from(material: ArtifactMaterial) -> Self {
        Self::Material(material)
    }
}

/// Mints artifact attachment capabilities for the durable artifact-store boundary.
///
/// Workspace architecture guards restrict construction to `actingcommand-artifact-store` and
/// contract tests. Transport metadata cannot be promoted back into this authority.
pub struct ArtifactStoreIssuer {
    identifiers: IdentifierIssuer,
}

impl ArtifactStoreIssuer {
    pub fn new() -> Result<Self, IdentifierIssuanceError> {
        Ok(Self {
            identifiers: IdentifierIssuer::new()?,
        })
    }

    pub fn issue<'a>(
        &self,
        kind: ArtifactKind,
        links: ArtifactLinksDraft,
        input: impl Into<ArtifactIssueInput<'a>>,
        created_at_unix_ms: u64,
        policy: ArtifactIssuePolicy,
    ) -> Result<StoreIssuedArtifact, SanitizationError> {
        let material = match input.into() {
            ArtifactIssueInput::Bytes(bytes) => {
                let mut material = ArtifactMaterialAccumulator::default();
                material.update(bytes).map_err(|_| {
                    SanitizationError::new("invalid_artifact_byte_count", "byte_count")
                })?;
                material.finish()
            }
            ArtifactIssueInput::Material(material) => material,
        };
        if material.byte_count == 0 {
            return Err(SanitizationError::new(
                "invalid_artifact_byte_count",
                "byte_count",
            ));
        }
        if created_at_unix_ms == 0 {
            return Err(SanitizationError::new(
                "invalid_artifact_timestamp",
                "created_at_unix_ms",
            ));
        }
        let artifact_id = self
            .identifiers
            .mint_artifact_id()
            .map_err(|_| SanitizationError::new("artifact_id_issuance_failed", "artifact_id"))?
            .into_transport();
        let ArtifactMaterial { byte_count, sha256 } = material;
        let object_key = object_key_for(&artifact_id, kind, &sha256);
        let reference = ArtifactReference {
            artifact_id,
            kind,
            run_id: links.run_id.map(IssuedRunId::into_transport),
            frame_id: links.frame_id.map(IssuedFrameId::into_transport),
            correlation_id: links
                .correlation_id
                .map(IssuedCorrelationId::into_transport),
            object_key,
            media_type: kind.media_type(),
            byte_count,
            sha256,
            created_at_unix_ms,
            producer: policy.producer,
            retention_class: policy.retention_class,
            redaction_state: policy.redaction_state,
        };
        reference.validate()?;
        Ok(StoreIssuedArtifact { reference })
    }

    pub fn verify_existing(
        &self,
        projected: ProjectedArtifactReference,
        bytes: &[u8],
    ) -> Result<VerifiedArtifactReference, SanitizationError> {
        let material = ArtifactMaterial {
            byte_count: u64::try_from(bytes.len())
                .map_err(|_| SanitizationError::new("invalid_artifact_byte_count", "byte_count"))?,
            sha256: canonical_sha256(bytes),
        };
        self.verify_existing_material(projected, material)
    }

    pub fn verify_existing_material(
        &self,
        projected: ProjectedArtifactReference,
        material: ArtifactMaterial,
    ) -> Result<VerifiedArtifactReference, SanitizationError> {
        projected.validate()?;
        let object_key = projected
            .object_key
            .ok_or_else(|| SanitizationError::new("invalid_artifact_reference", "object_key"))?;
        if material.byte_count != projected.byte_count || material.sha256 != projected.sha256 {
            return Err(SanitizationError::new(
                "artifact_verification_failed",
                "artifact",
            ));
        }
        let reference = ArtifactReference {
            artifact_id: projected.artifact_id,
            kind: projected.kind,
            run_id: projected.run_id,
            frame_id: projected.frame_id,
            correlation_id: projected.correlation_id,
            object_key,
            media_type: projected.media_type,
            byte_count: projected.byte_count,
            sha256: projected.sha256,
            created_at_unix_ms: projected.created_at_unix_ms,
            producer: projected.producer,
            retention_class: projected.retention_class,
            redaction_state: projected.redaction_state,
        };
        reference.validate()?;
        Ok(VerifiedArtifactReference { reference })
    }
}

#[cfg(test)]
impl fmt::Debug for ArtifactStoreIssuer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ArtifactStoreBoundary(<opaque>)")
    }
}

#[cfg(test)]
pub(super) fn issue_pending_for_tests(
    kind: ArtifactKind,
    links: ArtifactLinksDraft,
    bytes: &[u8],
    created_at_unix_ms: u64,
) -> Result<StoreIssuedArtifact, SanitizationError> {
    ArtifactStoreIssuer::new()
        .map_err(|_| SanitizationError::new("artifact_id_issuance_failed", "artifact_id"))?
        .issue(
            kind,
            links,
            bytes,
            created_at_unix_ms,
            ArtifactIssuePolicy::new(
                ArtifactProducer::CaptureStore,
                kind.default_retention_class(),
                ArtifactRedactionState::Pending,
            ),
        )
}

/// An attachment capability returned by the artifact-store issuer. It is neither serializable nor
/// deserializable, so a transport reference cannot be promoted back into producer ingress.
#[derive(Clone, PartialEq, Eq)]
pub struct StoreIssuedArtifact {
    reference: ArtifactReference,
}

impl StoreIssuedArtifact {
    pub const fn reference(&self) -> &ArtifactReference {
        &self.reference
    }

    pub(crate) fn into_reference(self) -> ArtifactReference {
        self.reference
    }
}

impl fmt::Debug for StoreIssuedArtifact {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("StoreIssuedArtifact(<opaque>)")
    }
}

/// Non-serializable authority returned only after the artifact-store verifies persisted bytes.
#[derive(Clone, PartialEq, Eq)]
pub struct VerifiedArtifactReference {
    reference: ArtifactReference,
}

impl VerifiedArtifactReference {
    pub const fn reference(&self) -> &ArtifactReference {
        &self.reference
    }

    pub fn into_reference(self) -> ArtifactReference {
        self.reference
    }
}

impl fmt::Debug for VerifiedArtifactReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("VerifiedArtifactReference(<opaque>)")
    }
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct ArtifactReference {
    artifact_id: ArtifactId,
    kind: ArtifactKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    run_id: Option<RunId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    frame_id: Option<FrameId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    correlation_id: Option<CorrelationId>,
    object_key: String,
    media_type: ArtifactMediaType,
    byte_count: u64,
    sha256: String,
    created_at_unix_ms: u64,
    producer: ArtifactProducer,
    retention_class: RetentionClass,
    redaction_state: ArtifactRedactionState,
}

impl ArtifactReference {
    pub const fn artifact_id(&self) -> &ArtifactId {
        &self.artifact_id
    }

    pub const fn kind(&self) -> ArtifactKind {
        self.kind
    }

    pub const fn run_id(&self) -> Option<&RunId> {
        self.run_id.as_ref()
    }

    pub const fn frame_id(&self) -> Option<&FrameId> {
        self.frame_id.as_ref()
    }

    pub const fn correlation_id(&self) -> Option<&CorrelationId> {
        self.correlation_id.as_ref()
    }

    pub fn object_key(&self) -> &str {
        &self.object_key
    }

    pub const fn media_type(&self) -> ArtifactMediaType {
        self.media_type
    }

    pub const fn byte_count(&self) -> u64 {
        self.byte_count
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    pub const fn created_at_unix_ms(&self) -> u64 {
        self.created_at_unix_ms
    }

    pub const fn producer(&self) -> ArtifactProducer {
        self.producer
    }

    pub const fn retention_class(&self) -> RetentionClass {
        self.retention_class
    }

    pub const fn redaction_state(&self) -> ArtifactRedactionState {
        self.redaction_state
    }

    pub const fn sensitivity(&self) -> Sensitivity {
        match self.redaction_state {
            ArtifactRedactionState::Pending => Sensitivity::Secret,
            ArtifactRedactionState::Applied => Sensitivity::Sensitive,
            ArtifactRedactionState::NotRequired => Sensitivity::Internal,
        }
    }

    pub fn validate(&self) -> Result<(), SanitizationError> {
        let valid = self.byte_count > 0
            && self.created_at_unix_ms > 0
            && is_sha256(&self.sha256)
            && self.media_type == self.kind.media_type()
            && self.object_key == object_key_for(&self.artifact_id, self.kind, &self.sha256);
        if valid {
            Ok(())
        } else {
            Err(SanitizationError::new(
                "invalid_artifact_reference",
                "artifact",
            ))
        }
    }

    pub fn project(&self, include_object_key: bool) -> ProjectedArtifactReference {
        ProjectedArtifactReference {
            artifact_id: self.artifact_id,
            kind: self.kind,
            run_id: self.run_id,
            frame_id: self.frame_id,
            correlation_id: self.correlation_id,
            object_key: include_object_key.then(|| self.object_key.clone()),
            media_type: self.media_type,
            byte_count: self.byte_count,
            sha256: self.sha256.clone(),
            created_at_unix_ms: self.created_at_unix_ms,
            producer: self.producer,
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
            .field("media_type", &self.media_type)
            .field("byte_count", &self.byte_count)
            .field("sha256", &"<redacted-digest>")
            .field("retention_class", &self.retention_class)
            .field("redaction_state", &self.redaction_state)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectedArtifactReference {
    pub artifact_id: ArtifactId,
    pub kind: ArtifactKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame_id: Option<FrameId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<CorrelationId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object_key: Option<String>,
    pub media_type: ArtifactMediaType,
    pub byte_count: u64,
    pub sha256: String,
    pub created_at_unix_ms: u64,
    pub producer: ArtifactProducer,
    pub retention_class: RetentionClass,
    pub redaction_state: ArtifactRedactionState,
}

impl ProjectedArtifactReference {
    pub fn validate(&self) -> Result<(), SanitizationError> {
        let object_key_valid = self.object_key.as_ref().is_none_or(|object_key| {
            object_key == &object_key_for(&self.artifact_id, self.kind, &self.sha256)
        });
        let valid = self.byte_count > 0
            && self.created_at_unix_ms > 0
            && is_sha256(&self.sha256)
            && self.media_type == self.kind.media_type()
            && object_key_valid;
        if valid {
            Ok(())
        } else {
            Err(SanitizationError::new(
                "invalid_projected_artifact_reference",
                "artifact",
            ))
        }
    }

    pub fn object_key(&self) -> Option<&str> {
        self.object_key.as_deref()
    }

    pub const fn kind(&self) -> ArtifactKind {
        self.kind
    }

    pub const fn frame_id(&self) -> Option<&FrameId> {
        self.frame_id.as_ref()
    }

    pub const fn media_type(&self) -> ArtifactMediaType {
        self.media_type
    }

    pub const fn byte_count(&self) -> u64 {
        self.byte_count
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    pub const fn redaction_state(&self) -> ArtifactRedactionState {
        self.redaction_state
    }
}

impl fmt::Debug for ProjectedArtifactReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProjectedArtifactReference")
            .field("artifact_id", &self.artifact_id)
            .field("kind", &self.kind)
            .field(
                "object_key",
                &self.object_key.as_ref().map(|_| "<redacted>"),
            )
            .field("media_type", &self.media_type)
            .field("byte_count", &self.byte_count)
            .field("sha256", &"<redacted-digest>")
            .field("retention_class", &self.retention_class)
            .field("redaction_state", &self.redaction_state)
            .finish()
    }
}

fn object_key_for(artifact_id: &ArtifactId, kind: ArtifactKind, sha256: &str) -> String {
    let shard = &sha256[7..9];
    format!(
        "artifacts/{shard}/{}.{}",
        artifact_id.canonical(),
        kind.extension()
    )
}

fn is_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

fn canonical_sha256(bytes: &[u8]) -> String {
    let digest = sha256(bytes);
    let mut value = String::with_capacity(71);
    value.push_str("sha256:");
    for byte in digest {
        use std::fmt::Write as _;
        write!(value, "{byte:02x}").expect("writing to a String cannot fail");
    }
    value
}

// FIPS 180-4 SHA-256 compression. This keeps the contract dependency budget unchanged.
fn sha256(input: &[u8]) -> [u8; 32] {
    const INITIAL: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    const ROUND: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    let bit_len = (input.len() as u64).wrapping_mul(8);
    let mut padded = Vec::with_capacity(input.len() + 72);
    padded.extend_from_slice(input);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    let mut state = INITIAL;
    for chunk in padded.as_chunks::<64>().0 {
        let mut schedule = [0_u32; 64];
        for (index, word) in schedule.iter_mut().take(16).enumerate() {
            let offset = index * 4;
            *word = u32::from_be_bytes([
                chunk[offset],
                chunk[offset + 1],
                chunk[offset + 2],
                chunk[offset + 3],
            ]);
        }
        for index in 16..64 {
            let s0 = schedule[index - 15].rotate_right(7)
                ^ schedule[index - 15].rotate_right(18)
                ^ (schedule[index - 15] >> 3);
            let s1 = schedule[index - 2].rotate_right(17)
                ^ schedule[index - 2].rotate_right(19)
                ^ (schedule[index - 2] >> 10);
            schedule[index] = schedule[index - 16]
                .wrapping_add(s0)
                .wrapping_add(schedule[index - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for index in 0..64 {
            let upper_e = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choose = (e & f) ^ ((!e) & g);
            let temporary_one = h
                .wrapping_add(upper_e)
                .wrapping_add(choose)
                .wrapping_add(ROUND[index])
                .wrapping_add(schedule[index]);
            let upper_a = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temporary_two = upper_a.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temporary_one);
            d = c;
            c = b;
            b = a;
            a = temporary_one.wrapping_add(temporary_two);
        }
        for (slot, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }

    let mut digest = [0_u8; 32];
    for (index, word) in state.into_iter().enumerate() {
        digest[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    digest
}
