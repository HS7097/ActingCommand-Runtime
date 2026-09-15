// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::{
    ArtifactEvictionDisposition, ArtifactEvictionObservation, ArtifactId, LedgerEventPosition,
    Sensitivity,
};

pub const MAX_RUNTIME_MATERIAL_CHUNK_BYTES: u32 = 64 * 1024;
pub const MAX_RUNTIME_MATERIAL_REPLY_BYTES: usize = 1024 * 1024;
pub const RUNTIME_MATERIAL_READ_BUDGET_MS: u64 = 4_000;

/// Selects one committed reference; no filesystem path or material authority comes from the caller.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeMaterialReadRequest {
    pub event: LedgerEventPosition,
    pub artifact_id: ArtifactId,
    pub snapshot_position: u64,
    pub byte_count: u64,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_run_id: Option<RunId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_frame_id: Option<FrameId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_request_id: Option<RequestId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_correlation_id: Option<CorrelationId>,
    pub offset: u64,
    pub requested_length: u32,
    pub max_reply_bytes: usize,
}

impl RuntimeMaterialReadRequest {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        validate_material_read_selection(
            self.event,
            self.snapshot_position,
            self.byte_count,
            &self.sha256,
        )?;
        if self.offset >= self.byte_count
            || !(1..=MAX_RUNTIME_MATERIAL_CHUNK_BYTES).contains(&self.requested_length)
            || self
                .offset
                .checked_add(u64::from(self.requested_length))
                .is_none()
            || !(1..=MAX_RUNTIME_MATERIAL_REPLY_BYTES).contains(&self.max_reply_bytes)
        {
            return Err(RuntimeContractError::new("invalid_material_read_request"));
        }
        Ok(())
    }
}

/// Validates the shared selection fields before Ledger authenticates the event, reference and links.
pub fn validate_material_read_selection(
    event: LedgerEventPosition,
    snapshot_position: u64,
    byte_count: u64,
    sha256: &str,
) -> RuntimeContractResult<()> {
    if event.sequence == 0
        || event.sequence > snapshot_position
        || byte_count == 0
        || !material_hash(sha256)
    {
        return Err(RuntimeContractError::new("invalid_material_read_request"));
    }
    Ok(())
}

impl fmt::Debug for RuntimeMaterialReadRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeMaterialReadRequest")
            .field("event", &self.event)
            .field("artifact_id", &self.artifact_id)
            .field("snapshot_position", &self.snapshot_position)
            .field("offset", &self.offset)
            .field("requested_length", &self.requested_length)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeMaterialReadSource {
    pub reference: ProjectedArtifactReference,
    pub sensitivity: Sensitivity,
    pub request_id: Option<RequestId>,
    pub run_id: Option<RunId>,
    pub frame_id: Option<FrameId>,
    pub correlation_id: Option<CorrelationId>,
    pub availability_through: u64,
    pub eviction: Option<ArtifactEvictionObservation>,
}

impl RuntimeMaterialReadSource {
    pub fn validate_for(&self, request: &RuntimeMaterialReadRequest) -> RuntimeContractResult<()> {
        self.reference
            .validate()
            .map_err(|_| RuntimeContractError::new("invalid_material_read_reference"))?;
        if self.reference.object_key.is_some()
            || self.reference.artifact_id != request.artifact_id
            || self.reference.byte_count != request.byte_count
            || self.reference.sha256 != request.sha256
            || self.availability_through < request.snapshot_position
            || request
                .expected_request_id
                .is_some_and(|id| self.request_id != Some(id))
            || request
                .expected_run_id
                .is_some_and(|id| self.run_id != Some(id))
            || request
                .expected_frame_id
                .is_some_and(|id| self.frame_id != Some(id))
            || request
                .expected_correlation_id
                .is_some_and(|id| self.correlation_id != Some(id))
        {
            return Err(RuntimeContractError::new("material_read_source_mismatch"));
        }
        if let Some(eviction) = &self.eviction {
            eviction
                .validate()
                .map_err(|_| RuntimeContractError::new("invalid_material_read_retention"))?;
            if eviction.artifact_id != request.artifact_id
                || eviction.through_sequence > self.availability_through
            {
                return Err(RuntimeContractError::new(
                    "material_read_retention_mismatch",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeMaterialReadState {
    Verified,
    NotProvided,
    Missing,
    IntegrityFailed,
    SourceIncomplete,
    RequestDenied,
    ReadFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeMaterialReadLimit {
    PendingEviction,
    Evicted,
    EvictionFailed,
    BudgetExceeded,
    ReplyBytes,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeMaterialChunk {
    pub offset: u64,
    pub actual_length: u32,
    pub total_length: u64,
    pub sha256: String,
    pub is_last: bool,
    pub bytes: Vec<u8>,
}

impl fmt::Debug for RuntimeMaterialChunk {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeMaterialChunk")
            .field("offset", &self.offset)
            .field("actual_length", &self.actual_length)
            .field("total_length", &self.total_length)
            .field("is_last", &self.is_last)
            .finish_non_exhaustive()
    }
}

/// Safe error codes and the original projection; native error text stays with its owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeMaterialReadFailure {
    pub code: String,
    pub operation: String,
    pub error: RuntimeErrorProjection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeMaterialReadResult {
    pub request: RuntimeMaterialReadRequest,
    pub source: Option<RuntimeMaterialReadSource>,
    pub state: RuntimeMaterialReadState,
    pub limit: Option<RuntimeMaterialReadLimit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk: Option<RuntimeMaterialChunk>,
    pub failure: Option<RuntimeMaterialReadFailure>,
}

impl RuntimeMaterialReadResult {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        self.request.validate()?;
        if let Some(source) = &self.source {
            source.validate_for(&self.request)?;
        }
        if let Some(failure) = &self.failure {
            for value in [&failure.code, &failure.operation] {
                if value.is_empty()
                    || value.len() > 128
                    || !value
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                {
                    return Err(RuntimeContractError::new("invalid_material_read_failure"));
                }
            }
        }
        match self.state {
            RuntimeMaterialReadState::Verified => {
                let source = self
                    .source
                    .as_ref()
                    .ok_or_else(|| RuntimeContractError::new("material_read_source_missing"))?;
                let chunk = self
                    .chunk
                    .as_ref()
                    .ok_or_else(|| RuntimeContractError::new("material_read_chunk_missing"))?;
                let expected_length = u64::from(self.request.requested_length)
                    .min(self.request.byte_count - self.request.offset);
                if source.eviction.is_some()
                    || self.failure.is_some()
                    || self.limit.is_some()
                    || chunk.offset != self.request.offset
                    || u64::from(chunk.actual_length) != expected_length
                    || chunk.bytes.len() != expected_length as usize
                    || chunk.total_length != self.request.byte_count
                    || chunk.sha256 != self.request.sha256
                    || chunk.is_last != (chunk.offset + expected_length == chunk.total_length)
                {
                    return Err(RuntimeContractError::new("invalid_verified_material_chunk"));
                }
            }
            RuntimeMaterialReadState::NotProvided => {
                if self.chunk.is_some() || self.limit.is_none() {
                    return Err(RuntimeContractError::new("invalid_material_read_limit"));
                }
                let disposition = self
                    .source
                    .as_ref()
                    .and_then(|source| source.eviction.as_ref());
                let valid = match self.limit {
                    Some(RuntimeMaterialReadLimit::PendingEviction) => {
                        disposition.is_some_and(|value| value.disposition.is_none())
                            && self.failure.is_none()
                    }
                    Some(RuntimeMaterialReadLimit::Evicted) => {
                        self.failure.is_none()
                            && disposition.is_some_and(|value| {
                                matches!(
                                    value.disposition,
                                    Some(
                                        ArtifactEvictionDisposition::Deleted
                                            | ArtifactEvictionDisposition::RecoveryAbsent
                                    )
                                )
                            })
                    }
                    Some(RuntimeMaterialReadLimit::EvictionFailed) => {
                        disposition.is_some_and(|value| {
                            value.disposition == Some(ArtifactEvictionDisposition::Failed)
                        }) && self.failure.is_none()
                    }
                    Some(
                        RuntimeMaterialReadLimit::BudgetExceeded
                        | RuntimeMaterialReadLimit::ReplyBytes,
                    ) => self.failure.is_some(),
                    None => false,
                };
                if !valid {
                    return Err(RuntimeContractError::new("material_read_limit_mismatch"));
                }
            }
            _ => {
                if self.chunk.is_some() || self.failure.is_none() || self.limit.is_some() {
                    return Err(RuntimeContractError::new("invalid_failed_material_read"));
                }
            }
        }
        Ok(())
    }

    pub fn receipt_state(&self) -> RuntimeReceiptState {
        if self.failure.is_none() {
            RuntimeReceiptState::Completed
        } else if self.state == RuntimeMaterialReadState::RequestDenied {
            RuntimeReceiptState::Denied
        } else {
            RuntimeReceiptState::Failed
        }
    }
}

fn material_hash(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}
