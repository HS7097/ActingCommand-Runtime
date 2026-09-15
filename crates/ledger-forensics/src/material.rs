// SPDX-License-Identifier: AGPL-3.0-only

use super::{ForensicError, ForensicReport, ForensicResult};
use actingcommand_artifact_store::{ArtifactStoreError, open_projected_stream};
use actingcommand_contract::{
    ArtifactEvictionDisposition, RUNTIME_MATERIAL_READ_BUDGET_MS, RuntimeErrorCode,
    RuntimeErrorProjection, RuntimeMaterialChunk, RuntimeMaterialReadFailure,
    RuntimeMaterialReadLimit, RuntimeMaterialReadRequest, RuntimeMaterialReadResult,
    RuntimeMaterialReadSource, RuntimeMaterialReadState, validate_material_read_selection,
};
use actingcommand_ledger::{
    GlobalLedger, GlobalLedgerError, GlobalLedgerEvidenceConfig, LedgerArtifactSelection,
    ResolvedLedgerArtifact,
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
            return Err(material_error("invalid_state_root", "state root is empty"));
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
    Identity(&'static str),
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
                "invalid_state_root",
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
                "invalid_material_read_limit",
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
            return Err(ReadError::Identity("material_read_reference_changed"));
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
            return Err(ReadError::Identity("material_read_verification_mismatch"));
        }
        if u64::try_from(bytes.len()).ok() != Some(selection.byte_count) {
            return Err(ReadError::Identity("material_read_length_overflow"));
        }
        if Instant::now() >= deadline {
            return Err(ReadError::Budget(material_error(
                "material_read_budget_exceeded",
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

/// One offline request opens fresh authenticated metadata twice, holding only the original
/// ArtifactStore reader across the second lookup and full material verification.
pub fn read_material_to<W: Write>(
    input: ForensicMaterialRequest,
    output: &mut W,
) -> ForensicResult<()> {
    let deadline = Instant::now()
        .checked_add(Duration::from_millis(RUNTIME_MATERIAL_READ_BUDGET_MS))
        .ok_or_else(|| material_error("material_read_deadline_overflow", "deadline overflow"))?;
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
            return Err(ReadError::Identity("material_read_reference_changed"));
        }
        result.source = Some(source(&current));
        if retained(&mut result) {
            return Ok(());
        }
        let range = reader
            .read_verified_range(
                result.request.offset,
                result.request.requested_length,
                deadline,
            )
            .map_err(ReadError::Artifact)?;
        let (verified, offset, bytes) = range.into_parts();
        if verified.reference().project(true) != *current.reference() {
            return Err(ReadError::Identity("material_read_verification_mismatch"));
        }
        let actual_length = u32::try_from(bytes.len())
            .map_err(|_| ReadError::Identity("material_read_length_overflow"))?;
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
            "material_read_encode_failed",
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
        let code = if limit == RuntimeMaterialReadLimit::ReplyBytes {
            "material_read_reply_limit"
        } else {
            "material_read_budget_exceeded"
        };
        let ForensicReport::MaterialRead(result) = &mut report else {
            unreachable!()
        };
        result.chunk = None;
        result.state = RuntimeMaterialReadState::NotProvided;
        result.limit = Some(limit);
        result.failure = Some(RuntimeMaterialReadFailure {
            code: code.to_owned(),
            operation: "read_ledger_material".to_owned(),
            error: RuntimeErrorProjection::new(RuntimeErrorCode::InvalidRequest, false),
        });
        result
            .validate()
            .map_err(|error| material_error(error.code(), "invalid bounded material result"))?;
        native_error = Some(material_error(
            code,
            "cooperative read or complete reply bound exceeded",
        ));
        body = serde_json::to_vec(&report).map_err(|_| {
            material_error(
                "material_read_encode_failed",
                "cannot encode material result",
            )
        })?;
    }
    if body.len() > maximum_reply_bytes {
        return Err(native_error.unwrap_or_else(|| {
            material_error(
                "material_read_reply_limit",
                "complete failure result does not fit reply bound",
            )
        }));
    }
    output
        .write_all(&body)
        .and_then(|()| output.write_all(b"\n"))
        .and_then(|()| output.flush())
        .map_err(|error| material_error("material_read_output_failed", &error.to_string()))?;
    if let Some(error) = native_error {
        return Err(error);
    }
    if matches!(&report, ForensicReport::MaterialRead(result) if result.state != RuntimeMaterialReadState::Verified)
    {
        return Err(material_error(
            "material_not_provided",
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

fn material_error(code: &'static str, detail: &str) -> ForensicError {
    ForensicError::new(code, "read_ledger_material", detail)
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
            let (state, limit) = match error.code() {
                "ledger_read_budget_exceeded" => (
                    RuntimeMaterialReadState::NotProvided,
                    Some(RuntimeMaterialReadLimit::BudgetExceeded),
                ),
                "ledger_source_incomplete" => (RuntimeMaterialReadState::SourceIncomplete, None),
                _ if error.is_fatal() => (RuntimeMaterialReadState::ReadFailed, None),
                _ => (RuntimeMaterialReadState::RequestDenied, None),
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
            let state = if matches!(
                error.code(),
                "artifact_read_budget_exceeded" | "artifact_read_material_limit"
            ) {
                RuntimeMaterialReadState::NotProvided
            } else if error.code() == "artifact_read_failed"
                && error.io_error_kind() == Some(std::io::ErrorKind::NotFound)
            {
                RuntimeMaterialReadState::Missing
            } else if matches!(
                error.code(),
                "artifact_hash_mismatch" | "artifact_verify_failed"
            ) {
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
            code,
            "read_ledger_material",
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
        ForensicError::new(code, operation, detail),
    )
}
