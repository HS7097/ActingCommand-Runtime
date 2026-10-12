// SPDX-License-Identifier: AGPL-3.0-only

use super::{ForensicError, ForensicReport, ForensicResult};
use crate::codes::{ForensicsCode, ForensicsLocation};
use actingcommand_artifact_store::{ArtifactStoreError, StoreCode, open_projected_stream};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::{Code, Location};
use actingcommand_contract::{
    ArtifactEvictionDisposition, RUNTIME_MATERIAL_READ_BUDGET_MS, RuntimeErrorCode,
    RuntimeErrorProjection, RuntimeMaterialChunk, RuntimeMaterialReadFailure,
    RuntimeMaterialReadLimit, RuntimeMaterialReadRequest, RuntimeMaterialReadResult,
    RuntimeMaterialReadSource, RuntimeMaterialReadState, validate_material_read_selection,
};
use actingcommand_ledger::{
    GlobalLedger, GlobalLedgerError, GlobalLedgerEvidenceConfig, LedgerArtifactSelection,
    LedgerCode, ResolvedLedgerArtifact,
};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub struct ForensicMaterialRequest {
    state_root: PathBuf,
    request: RuntimeMaterialReadRequest,
}

impl ForensicMaterialRequest {
    pub fn new(
        state_root: impl Into<PathBuf>,
        request: RuntimeMaterialReadRequest,
    ) -> ForensicResult<Self> {
        let state_root = state_root.into();
        if state_root.as_os_str().is_empty() {
            return Err(material_error(
                ForensicsCode::InvalidStateRoot.code(),
                "state root is empty",
            ));
        }
        request
            .validate()
            .map_err(|error| material_error(error.code(), "invalid typed material selection"))?;
        Ok(Self {
            state_root,
            request,
        })
    }
}

enum ReadError {
    Ledger(GlobalLedgerError),
    Artifact(ArtifactStoreError),
    Identity(Code),
    Request(ForensicError),
    Budget(ForensicError),
}

/// A complete offline material result. Only Verified owns bytes; failures retain their source.
#[must_use = "inspect material availability and failure before consuming the result"]
pub enum ForensicMaterialCompleteResult {
    Verified {
        source: RuntimeMaterialReadSource,
        bytes: Vec<u8>,
    },
    NotProvided {
        source: Option<RuntimeMaterialReadSource>,
        limit: RuntimeMaterialReadLimit,
        failure: Option<RuntimeMaterialReadFailure>,
        error: Option<ForensicError>,
    },
    Failed {
        source: Option<RuntimeMaterialReadSource>,
        state: RuntimeMaterialReadState,
        failure: RuntimeMaterialReadFailure,
        error: ForensicError,
    },
}

/// Opens fresh authenticated metadata twice and reads one whole object under the original reader.
/// No prefix is exposed, and the absolute deadline is cooperative around synchronous I/O.
pub fn read_material_complete(
    state_root: impl AsRef<Path>,
    selection: LedgerArtifactSelection,
    max_material_bytes: usize,
    deadline: Instant,
) -> ForensicMaterialCompleteResult {
    let state_root = state_root.as_ref();
    let mut actual_source = None;
    let work = (|| -> Result<ForensicMaterialCompleteResult, ReadError> {
        if state_root.as_os_str().is_empty() {
            return Err(ReadError::Request(material_error(
                ForensicsCode::InvalidStateRoot.code(),
                "state root is empty",
            )));
        }
        validate_material_read_selection(
            selection.event,
            selection.snapshot_position,
            selection.byte_count,
            &selection.sha256,
        )
        .map_err(|error| {
            ReadError::Request(material_error(
                error.code(),
                "invalid typed material selection",
            ))
        })?;
        if max_material_bytes == 0 || max_material_bytes > isize::MAX as usize {
            return Err(ReadError::Request(material_error(
                SharedCode::InvalidMaterialReadLimit.code(),
                "complete material requires a positive owned-buffer bound within isize::MAX",
            )));
        }
        let snapshot = GlobalLedger::open_metadata(
            GlobalLedgerEvidenceConfig::new(state_root).with_deadline(deadline),
        )
        .map_err(ReadError::Ledger)?;
        let original = snapshot
            .resolve_artifact(&selection, deadline)
            .map_err(ReadError::Ledger)?;
        drop(snapshot);
        actual_source = Some(source(&original));
        if let Some(limit) = actual_source.as_ref().and_then(retention_limit) {
            return Ok(ForensicMaterialCompleteResult::NotProvided {
                source: actual_source.take(),
                limit,
                failure: None,
                error: None,
            });
        }
        let reader =
            open_projected_stream(state_root, original.reference()).map_err(ReadError::Artifact)?;
        let current_snapshot = GlobalLedger::open_metadata(
            GlobalLedgerEvidenceConfig::new(state_root).with_deadline(deadline),
        )
        .map_err(ReadError::Ledger)?;
        let current = current_snapshot
            .resolve_artifact(&selection, deadline)
            .map_err(ReadError::Ledger)?;
        drop(current_snapshot);
        if original.reference() != current.reference()
            || original.links() != current.links()
            || original.event() != current.event()
        {
            return Err(ReadError::Identity(
                SharedCode::MaterialReadReferenceChanged.code(),
            ));
        }
        actual_source = Some(source(&current));
        if let Some(limit) = actual_source.as_ref().and_then(retention_limit) {
            return Ok(ForensicMaterialCompleteResult::NotProvided {
                source: actual_source.take(),
                limit,
                failure: None,
                error: None,
            });
        }
        let (verified, bytes) = reader
            .read_verified_complete(max_material_bytes, deadline)
            .map_err(ReadError::Artifact)?;
        if verified.reference().project(true) != *current.reference() {
            return Err(ReadError::Identity(
                SharedCode::MaterialReadVerificationMismatch.code(),
            ));
        }
        if u64::try_from(bytes.len()).ok() != Some(selection.byte_count) {
            return Err(ReadError::Identity(
                SharedCode::MaterialReadLengthOverflow.code(),
            ));
        }
        if Instant::now() >= deadline {
            return Err(ReadError::Budget(material_error(
                SharedCode::MaterialReadBudgetExceeded.code(),
                "cooperative material read deadline expired",
            )));
        }
        Ok(ForensicMaterialCompleteResult::Verified {
            source: source(&current),
            bytes,
        })
    })();
    match work {
        Ok(result) => result,
        Err(error) => {
            let (state, limit, failure, error) = read_failure(error);
            match limit {
                Some(limit) if state == RuntimeMaterialReadState::NotProvided => {
                    ForensicMaterialCompleteResult::NotProvided {
                        source: actual_source,
                        limit,
                        failure: Some(failure),
                        error: Some(error),
                    }
                }
                _ => ForensicMaterialCompleteResult::Failed {
                    source: actual_source,
                    state,
                    failure,
                    error,
                },
            }
        }
    }
}

// Workflow #395 (LEDG-395): the phases of one offline `actingledger material` read.
/// The phases of one offline `material` read, in order (Workflow #395).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForensicMaterialPhase {
    /// Opens authenticated metadata and resolves the selected reference and its retention.
    Reference,
    /// Under the reader's use guard, reopens the metadata and confirms the same reference and
    /// its current retention.
    CurrentReference,
    /// Reads and verifies the material and encodes the reply.
    VerifiedRead,
}

impl ForensicMaterialPhase {
    /// The phase's registered location.
    pub const fn location(self) -> Location {
        match self {
            Self::Reference => ForensicsLocation::ResolveMaterialReference.location(),
            Self::CurrentReference => ForensicsLocation::ResolveCurrentMaterialReference.location(),
            Self::VerifiedRead => ForensicsLocation::ReadVerifiedMaterial.location(),
        }
    }
}

/// One offline request opens fresh authenticated metadata twice, holding only the original
/// ArtifactStore reader across the second lookup and full material verification. Each of its
/// three phases gets `RUNTIME_MATERIAL_READ_BUDGET_MS` from its own start (Workflow #395).
pub fn read_material_to<W: Write>(
    input: ForensicMaterialRequest,
    output: &mut W,
) -> ForensicResult<()> {
    read_material_to_per_phase(input, output, |_| {
        Duration::from_millis(RUNTIME_MATERIAL_READ_BUDGET_MS)
    })
}

/// `read_material_to` with the budget of each phase taken from `budget` (Workflow #395). A
/// phase's deadline is its budget from the moment the phase starts, so a slow phase leaves the
/// next one its whole budget. A deadline that runs out names its phase as the failure's
/// location, in the report and in the returned error.
pub fn read_material_to_per_phase<W: Write>(
    input: ForensicMaterialRequest,
    output: &mut W,
    mut budget: impl FnMut(ForensicMaterialPhase) -> Duration,
) -> ForensicResult<()> {
    let mut phase = ForensicMaterialPhase::Reference;
    let mut deadline = phase_deadline(phase, &mut budget)?;
    let request = input.request;
    let selection = LedgerArtifactSelection {
        event: request.event,
        artifact_id: request.artifact_id,
        snapshot_position: request.snapshot_position,
        sha256: request.sha256.clone(),
        byte_count: request.byte_count,
        run_id: request.expected_run_id,
        frame_id: request.expected_frame_id,
        request_id: request.expected_request_id,
        correlation_id: request.expected_correlation_id,
    };
    let mut result = RuntimeMaterialReadResult {
        request,
        source: None,
        state: RuntimeMaterialReadState::ReadFailed,
        limit: None,
        chunk: None,
        failure: None,
    };
    let work = (|| -> Result<(), ReadError> {
        let snapshot = GlobalLedger::open_metadata(
            GlobalLedgerEvidenceConfig::new(&input.state_root).with_deadline(deadline),
        )
        .map_err(ReadError::Ledger)?;
        let original = snapshot
            .resolve_artifact(&selection, deadline)
            .map_err(ReadError::Ledger)?;
        drop(snapshot);
        result.source = Some(source(&original));
        if retained(&mut result) {
            return Ok(());
        }
        phase = ForensicMaterialPhase::CurrentReference;
        deadline = phase_deadline(phase, &mut budget).map_err(ReadError::Request)?;
        let reader = open_projected_stream(&input.state_root, original.reference())
            .map_err(ReadError::Artifact)?;
        let current_snapshot = GlobalLedger::open_metadata(
            GlobalLedgerEvidenceConfig::new(&input.state_root).with_deadline(deadline),
        )
        .map_err(ReadError::Ledger)?;
        let current = current_snapshot
            .resolve_artifact(&selection, deadline)
            .map_err(ReadError::Ledger)?;
        drop(current_snapshot);
        if original.reference() != current.reference()
            || original.links() != current.links()
            || original.event() != current.event()
        {
            return Err(ReadError::Identity(
                SharedCode::MaterialReadReferenceChanged.code(),
            ));
        }
        result.source = Some(source(&current));
        if retained(&mut result) {
            return Ok(());
        }
        phase = ForensicMaterialPhase::VerifiedRead;
        deadline = phase_deadline(phase, &mut budget).map_err(ReadError::Request)?;
        let range = reader
            .read_verified_range(
                result.request.offset,
                result.request.requested_length,
                deadline,
            )
            .map_err(ReadError::Artifact)?;
        let (verified, offset, bytes) = range.into_parts();
        if verified.reference().project(true) != *current.reference() {
            return Err(ReadError::Identity(
                SharedCode::MaterialReadVerificationMismatch.code(),
            ));
        }
        let actual_length = u32::try_from(bytes.len())
            .map_err(|_| ReadError::Identity(SharedCode::MaterialReadLengthOverflow.code()))?;
        result.state = RuntimeMaterialReadState::Verified;
        result.chunk = Some(RuntimeMaterialChunk {
            offset,
            actual_length,
            total_length: result.request.byte_count,
            sha256: result.request.sha256.clone(),
            is_last: offset.checked_add(u64::from(actual_length))
                == Some(result.request.byte_count),
            bytes,
        });
        Ok(())
    })();
    let mut native_error = None;
    if let Err(error) = work {
        let (state, limit, failure, error) = read_failure(error);
        let (failure, error) = if limit == Some(RuntimeMaterialReadLimit::BudgetExceeded) {
            at_phase(failure, error, phase)
        } else {
            (failure, error)
        };
        native_error = Some(error);
        result.state = state;
        result.limit = limit;
        result.chunk = None;
        result.failure = Some(failure);
    }
    result
        .validate()
        .map_err(|error| material_error(error.code(), "invalid material read result"))?;
    let maximum_reply_bytes = result.request.max_reply_bytes;
    let mut report = ForensicReport::MaterialRead(Box::new(result));
    let mut body = serde_json::to_vec(&report).map_err(|_| {
        material_error(
            ForensicsCode::MaterialReadEncodeFailed.code(),
            "cannot encode material result",
        )
    })?;
    let limit = if body.len() > maximum_reply_bytes {
        Some(RuntimeMaterialReadLimit::ReplyBytes)
    } else if Instant::now() >= deadline {
        Some(RuntimeMaterialReadLimit::BudgetExceeded)
    } else {
        None
    };
    if let Some(limit) = limit
        && native_error.is_none()
    {
        body.clear();
        // Workflow #395: a deadline that ran out names the phase it bounded.
        let (code, operation) = if limit == RuntimeMaterialReadLimit::ReplyBytes {
            (
                SharedCode::MaterialReadReplyLimit.code(),
                ForensicsLocation::ReadLedgerMaterial.location(),
            )
        } else {
            (
                SharedCode::MaterialReadBudgetExceeded.code(),
                phase.location(),
            )
        };
        let ForensicReport::MaterialRead(result) = &mut report else {
            unreachable!()
        };
        result.chunk = None;
        result.state = RuntimeMaterialReadState::NotProvided;
        result.limit = Some(limit);
        result.failure = Some(RuntimeMaterialReadFailure {
            code: code.as_str().to_owned(),
            operation: operation.as_str().to_owned(),
            error: RuntimeErrorProjection::new(RuntimeErrorCode::InvalidRequest, false),
        });
        result
            .validate()
            .map_err(|error| material_error(error.code(), "invalid bounded material result"))?;
        native_error = Some(ForensicError::new(
            code,
            operation,
            "cooperative read or complete reply bound exceeded",
        ));
        body = serde_json::to_vec(&report).map_err(|_| {
            material_error(
                ForensicsCode::MaterialReadEncodeFailed.code(),
                "cannot encode material result",
            )
        })?;
    }
    if body.len() > maximum_reply_bytes {
        return Err(native_error.unwrap_or_else(|| {
            material_error(
                SharedCode::MaterialReadReplyLimit.code(),
                "complete failure result does not fit reply bound",
            )
        }));
    }
    output
        .write_all(&body)
        .and_then(|()| output.write_all(b"\n"))
        .and_then(|()| output.flush())
        .map_err(|error| {
            material_error(
                ForensicsCode::MaterialReadOutputFailed.code(),
                &error.to_string(),
            )
        })?;
    if let Some(error) = native_error {
        return Err(error);
    }
    if matches!(&report, ForensicReport::MaterialRead(result) if result.state != RuntimeMaterialReadState::Verified)
    {
        return Err(material_error(
            ForensicsCode::MaterialNotProvided.code(),
            "see committed retention state in structured result",
        ));
    }
    Ok(())
}

fn source(resolved: &ResolvedLedgerArtifact) -> RuntimeMaterialReadSource {
    let mut reference = resolved.reference().clone();
    reference.object_key = None;
    RuntimeMaterialReadSource {
        reference,
        sensitivity: resolved.sensitivity(),
        request_id: resolved.links().request_id().copied(),
        run_id: resolved.links().run_id().copied(),
        frame_id: resolved.links().frame_id().copied(),
        correlation_id: resolved.links().correlation_id().copied(),
        availability_through: resolved.availability_through(),
        eviction: resolved.eviction().cloned(),
    }
}

fn retained(result: &mut RuntimeMaterialReadResult) -> bool {
    let Some(limit) = result.source.as_ref().and_then(retention_limit) else {
        return false;
    };
    result.state = RuntimeMaterialReadState::NotProvided;
    result.limit = Some(limit);
    true
}

fn retention_limit(source: &RuntimeMaterialReadSource) -> Option<RuntimeMaterialReadLimit> {
    let eviction = source.eviction.as_ref()?;
    Some(match eviction.disposition {
        None => RuntimeMaterialReadLimit::PendingEviction,
        Some(
            ArtifactEvictionDisposition::Deleted | ArtifactEvictionDisposition::RecoveryAbsent,
        ) => RuntimeMaterialReadLimit::Evicted,
        Some(ArtifactEvictionDisposition::Failed) => RuntimeMaterialReadLimit::EvictionFailed,
    })
}

/// Workflow #395: the deadline of `phase`, its budget from now.
fn phase_deadline(
    phase: ForensicMaterialPhase,
    budget: &mut impl FnMut(ForensicMaterialPhase) -> Duration,
) -> ForensicResult<Instant> {
    Instant::now().checked_add(budget(phase)).ok_or_else(|| {
        material_error(
            SharedCode::MaterialReadDeadlineOverflow.code(),
            "deadline overflow",
        )
    })
}

/// Workflow #395: a deadline that ran out names its phase as the failure's location.
fn at_phase(
    mut failure: RuntimeMaterialReadFailure,
    error: ForensicError,
    phase: ForensicMaterialPhase,
) -> (RuntimeMaterialReadFailure, ForensicError) {
    let location = phase.location().as_str();
    failure.operation = location.to_owned();
    (
        failure,
        ForensicError::relayed(error.code(), location, error.detail),
    )
}

fn material_error(code: Code, detail: &str) -> ForensicError {
    ForensicError::new(
        code,
        ForensicsLocation::ReadLedgerMaterial.location(),
        detail,
    )
}

fn read_failure(
    error: ReadError,
) -> (
    RuntimeMaterialReadState,
    Option<RuntimeMaterialReadLimit>,
    RuntimeMaterialReadFailure,
    ForensicError,
) {
    let (state, limit, code, operation, projection, detail) = match error {
        ReadError::Ledger(error) => {
            let failed = error.code();
            let (state, limit) = if failed == LedgerCode::LedgerReadBudgetExceeded.as_str() {
                (
                    RuntimeMaterialReadState::NotProvided,
                    Some(RuntimeMaterialReadLimit::BudgetExceeded),
                )
            } else if failed == LedgerCode::LedgerSourceIncomplete.as_str() {
                (RuntimeMaterialReadState::SourceIncomplete, None)
            } else if error.is_fatal() {
                (RuntimeMaterialReadState::ReadFailed, None)
            } else {
                (RuntimeMaterialReadState::RequestDenied, None)
            };
            (
                state,
                limit,
                error.code(),
                error.operation(),
                RuntimeErrorProjection::new(
                    if error.is_fatal() {
                        RuntimeErrorCode::LedgerFailure
                    } else {
                        RuntimeErrorCode::InvalidRequest
                    },
                    error.is_fatal(),
                ),
                error
                    .detail()
                    .unwrap_or("Ledger source request failed")
                    .to_owned(),
            )
        }
        ReadError::Artifact(error) => {
            let code = error.code();
            let state = if code == StoreCode::ArtifactReadBudgetExceeded.as_str()
                || code == StoreCode::ArtifactReadMaterialLimit.as_str()
            {
                RuntimeMaterialReadState::NotProvided
            } else if code == SharedCode::ArtifactReadFailed.as_str()
                && error.io_error_kind() == Some(std::io::ErrorKind::NotFound)
            {
                RuntimeMaterialReadState::Missing
            } else if code == StoreCode::ArtifactHashMismatch.as_str()
                || code == SharedCode::ArtifactVerifyFailed.as_str()
            {
                RuntimeMaterialReadState::IntegrityFailed
            } else {
                RuntimeMaterialReadState::ReadFailed
            };
            (
                state,
                (state == RuntimeMaterialReadState::NotProvided)
                    .then_some(RuntimeMaterialReadLimit::BudgetExceeded),
                error.code(),
                error.operation(),
                RuntimeErrorProjection::new(
                    if error.is_fatal() {
                        RuntimeErrorCode::RuntimeFatal
                    } else {
                        RuntimeErrorCode::InvalidRequest
                    },
                    error.is_fatal(),
                ),
                error.to_string(),
            )
        }
        ReadError::Identity(code) => (
            RuntimeMaterialReadState::IntegrityFailed,
            None,
            code.as_str(),
            ForensicsLocation::ReadLedgerMaterial.as_str(),
            RuntimeErrorProjection::new(RuntimeErrorCode::RuntimeFatal, true),
            "material source identity changed".to_owned(),
        ),
        ReadError::Request(error) => (
            RuntimeMaterialReadState::RequestDenied,
            None,
            error.code(),
            error.operation(),
            RuntimeErrorProjection::new(RuntimeErrorCode::InvalidRequest, false),
            error.detail,
        ),
        ReadError::Budget(error) => (
            RuntimeMaterialReadState::NotProvided,
            Some(RuntimeMaterialReadLimit::BudgetExceeded),
            error.code(),
            error.operation(),
            RuntimeErrorProjection::new(RuntimeErrorCode::InvalidRequest, false),
            error.detail,
        ),
    };
    (
        state,
        limit,
        RuntimeMaterialReadFailure {
            code: code.to_owned(),
            operation: operation.to_owned(),
            error: projection,
        },
        ForensicError::relayed(code, operation, detail),
    )
}
