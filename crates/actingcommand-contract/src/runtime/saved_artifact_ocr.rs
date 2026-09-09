// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

pub const SAVED_ARTIFACT_OCR_DEADLINE_MS: u64 = 120_000;

/// Exact historical source; these declarations require native ledger verification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedArtifactOcrSource {
    pub state_root: String,
    pub through_sequence: u64,
    pub frame_id: FrameId,
    pub artifact: ProjectedArtifactReference,
    pub created: TerminalEvent,
    pub verified: TerminalEvent,
    pub captured: TerminalEvent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedArtifactOcrRequest {
    pub source: SavedArtifactOcrSource,
    pub package_path: String,
    pub expected_sha256: crate::PackageRef,
    pub target_id: String,
}

impl SavedArtifactOcrRequest {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        let source = &self.source;
        if self.package_path.is_empty()
            || self.package_path.len() > 4096
            || source.state_root.is_empty()
            || source.state_root.len() > 4096
            || !std::path::Path::new(&self.package_path).is_absolute()
            || !std::path::Path::new(&source.state_root).is_absolute()
            || self.target_id.trim().is_empty()
            || self.target_id.len() > 4096
            || self.expected_sha256.validate().is_err()
            || source.artifact.validate().is_err()
            || source.artifact.kind != ArtifactKind::CaptureFrame
            || source.artifact.frame_id != Some(source.frame_id)
            || source.artifact.object_key.is_none()
            || source.artifact.byte_count > MAX_READONLY_OBSERVATION_ARTIFACT_BYTES
            || source.created.sequence == 0
            || source.created.sequence >= source.verified.sequence
            || source.verified.sequence >= source.captured.sequence
            || source.captured.sequence > source.through_sequence
        {
            return Err(RuntimeContractError::new(
                "invalid_saved_artifact_ocr_request",
            ));
        }
        Ok(())
    }
}

/// A new diagnostic artifact, never a new capture or a current policy fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedArtifactOcrResult {
    pub source: SavedArtifactOcrSource,
    pub target_id: String,
    pub artifact: ProjectedArtifactReference,
    pub verified: TerminalEvent,
}

impl SavedArtifactOcrResult {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.artifact.validate().is_err()
            || self.artifact.kind != ArtifactKind::DiagnosticJson
            || self.artifact.object_key.is_none()
            || self.artifact.frame_id.is_some()
            || self.artifact.run_id.is_some()
            || self.target_id.is_empty()
            || self.verified.sequence == 0
        {
            return Err(RuntimeContractError::new(
                "invalid_saved_artifact_ocr_result",
            ));
        }
        Ok(())
    }
}
