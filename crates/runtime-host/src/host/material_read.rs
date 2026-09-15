// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_artifact_store::open_projected_stream;
use actingcommand_contract::{
    ArtifactEvictionDisposition, MAX_RUNTIME_MATERIAL_REPLY_BYTES, RUNTIME_MATERIAL_READ_BUDGET_MS,
    RuntimeMaterialChunk, RuntimeMaterialReadFailure, RuntimeMaterialReadLimit,
    RuntimeMaterialReadRequest, RuntimeMaterialReadResult, RuntimeMaterialReadSource,
    RuntimeMaterialReadState,
};
use actingcommand_ledger::{GlobalLedgerError, LedgerArtifactSelection, ResolvedLedgerArtifact};

#[derive(Clone, Copy)]
pub(super) struct MaterialReadContext {
    pub(super) deadline: Instant,
    pub(super) max_reply_bytes: usize,
}

impl MaterialReadContext {
    pub(super) fn for_request(
        request: &RuntimeRequest,
        maximum_frame_bytes: usize,
    ) -> RuntimeHostResult<Option<Self>> {
        let RuntimeOperation::ReadMaterial { request } = request.operation() else {
            return Ok(None);
        };
        Ok(Some(Self {
            deadline: Instant::now()
                .checked_add(Duration::from_millis(RUNTIME_MATERIAL_READ_BUDGET_MS))
                .ok_or_else(|| protocol_error("material_read_deadline_overflow"))?,
            max_reply_bytes: request
                .max_reply_bytes
                .min(maximum_frame_bytes)
                .min(MAX_RUNTIME_MATERIAL_REPLY_BYTES),
        }))
    }
}

enum MaterialReadError {
    Ledger(GlobalLedgerError),
    Artifact(ArtifactStoreError),
    Identity(RuntimeHostError),
}

impl HostShared {
    pub(super) fn read_material(
        &self,
        validated: &ValidatedRuntimeRequest<'_>,
        request: &RuntimeMaterialReadRequest,
        context: MaterialReadContext,
    ) -> Result<OperationSuccess, RequestFailure> {
        let selection = material_selection(request);
        let mut result = RuntimeMaterialReadResult {
            request: request.clone(),
            source: None,
            state: RuntimeMaterialReadState::ReadFailed,
            limit: None,
            chunk: None,
            failure: None,
        };
        let work = (|| -> Result<(), MaterialReadError> {
            let original = self
                .ledger
                .resolve_artifact(selection.clone(), context.deadline)
                .map_err(MaterialReadError::Ledger)?;
            result.source = Some(material_source(&original));
            if set_retention_result(&mut result) {
                return Ok(());
            }
            let reader = open_projected_stream(self.artifacts.root(), original.reference())
                .map_err(MaterialReadError::Artifact)?;
            let current = self
                .ledger
                .resolve_artifact(selection, context.deadline)
                .map_err(MaterialReadError::Ledger)?;
            if current.reference() != original.reference()
                || current.event() != original.event()
                || current.links() != original.links()
            {
                return Err(MaterialReadError::Identity(RuntimeHostError::fatal(
                    "material_read_reference_changed",
                    "read_runtime_material",
                    RuntimeErrorCode::RuntimeFatal,
                )));
            }
            result.source = Some(material_source(&current));
            if set_retention_result(&mut result) {
                return Ok(());
            }
            let range = reader
                .read_verified_range(request.offset, request.requested_length, context.deadline)
                .map_err(MaterialReadError::Artifact)?;
            let (verified, offset, bytes) = range.into_parts();
            if verified.reference().project(true) != *current.reference() {
                return Err(MaterialReadError::Identity(RuntimeHostError::fatal(
                    "material_read_verification_mismatch",
                    "read_runtime_material",
                    RuntimeErrorCode::RuntimeFatal,
                )));
            }
            let length = u32::try_from(bytes.len()).map_err(|_| {
                MaterialReadError::Identity(protocol_error("material_read_length_overflow"))
            })?;
            result.state = RuntimeMaterialReadState::Verified;
            result.chunk = Some(RuntimeMaterialChunk {
                offset,
                actual_length: length,
                total_length: request.byte_count,
                sha256: request.sha256.clone(),
                is_last: offset.checked_add(u64::from(length)) == Some(request.byte_count),
                bytes,
            });
            Ok(())
        })();
        if let Err(error) = work {
            let (state, limit, error) = match error {
                MaterialReadError::Ledger(error) => {
                    let mut host = if error.is_fatal() {
                        RuntimeHostError::fatal(
                            error.code(),
                            error.operation(),
                            RuntimeErrorCode::LedgerFailure,
                        )
                    } else {
                        RuntimeHostError::request(
                            error.code(),
                            error.operation(),
                            RuntimeErrorCode::InvalidRequest,
                        )
                    };
                    host.lifecycle.native_detail = Some(Box::new(
                        ArtifactStoreError::fatal(
                            error.code(),
                            error.operation(),
                            error.detail().unwrap_or("Ledger request failed"),
                        )
                        .native_detail(),
                    ));
                    if error.is_fatal() {
                        return Err(RequestFailure::poison_without_terminal(host));
                    }
                    match error.code() {
                        "ledger_read_budget_exceeded" => (
                            RuntimeMaterialReadState::NotProvided,
                            Some(RuntimeMaterialReadLimit::BudgetExceeded),
                            host,
                        ),
                        "ledger_source_incomplete" => {
                            (RuntimeMaterialReadState::SourceIncomplete, None, host)
                        }
                        _ => (RuntimeMaterialReadState::RequestDenied, None, host),
                    }
                }
                MaterialReadError::Artifact(error) => {
                    let state = if error.code() == "artifact_read_budget_exceeded" {
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
                    let limit = (state == RuntimeMaterialReadState::NotProvided)
                        .then_some(RuntimeMaterialReadLimit::BudgetExceeded);
                    (state, limit, RuntimeHostError::artifact(error))
                }
                MaterialReadError::Identity(error) => {
                    (RuntimeMaterialReadState::IntegrityFailed, None, error)
                }
            };
            self.record_material_read_failure(validated, &error, request, result.source.is_some())
                .map_err(RequestFailure::poison_without_terminal)?;
            result.state = state;
            result.limit = limit;
            result.chunk = None;
            result.failure = Some(material_failure(&error));
        }
        result
            .validate()
            .map_err(|_| RequestFailure::poison_without_terminal(receipt_error()))?;
        Ok(OperationSuccess {
            state: result.receipt_state(),
            terminal: None,
            result: RuntimeResult::MaterialRead {
                result: Box::new(result),
            },
        })
    }

    fn record_material_read_failure(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        error: &RuntimeHostError,
        selection: &RuntimeMaterialReadRequest,
        reference_resolved: bool,
    ) -> RuntimeHostResult<()> {
        let error = error
            .clone()
            .with_diagnostic_detail(DiagnosticDetailDraft::new(
                "material_read",
                "attempt",
                "committed_reference",
                "read_runtime_material",
                serde_json::json!({
                    "event": selection.event,
                    "artifact_id": selection.artifact_id,
                    "snapshot_position": selection.snapshot_position,
                    "offset": selection.offset,
                    "requested_length": selection.requested_length,
                    "reference_resolved": reference_resolved,
                })
                .to_string(),
                Sensitivity::Internal,
            ));
        self.append_lifecycle_failure(
            RuntimeLifecycleFailureStage::OperationCleanup,
            RuntimeLifecycleFailure::Host(&error),
            request.event_links(None, None, None),
            reference_resolved.then_some(selection.event.event_id),
        )?;
        if error.is_fatal() {
            self.fatal.mark(error.clone())?;
        }
        Ok(())
    }

    /// The exact encoded receipt checked here is the one written by the existing frame writer.
    pub(super) fn prepare_material_reply(
        &self,
        request: &RuntimeRequest,
        mut receipt: RuntimeReceipt,
        context: MaterialReadContext,
    ) -> RuntimeHostResult<(RuntimeReceipt, Vec<u8>)> {
        let mut body = self.encode_material_reply(request, &receipt)?;
        let limit = if body.len() > context.max_reply_bytes {
            Some(RuntimeMaterialReadLimit::ReplyBytes)
        } else if Instant::now() >= context.deadline {
            Some(RuntimeMaterialReadLimit::BudgetExceeded)
        } else {
            None
        };
        if let Some(limit) = limit
            && receipt.error_projection().is_none()
            && matches!(receipt.result(), Some(RuntimeResult::MaterialRead { .. }))
        {
            body.clear();
            let code = if limit == RuntimeMaterialReadLimit::ReplyBytes {
                "material_read_reply_limit"
            } else {
                "material_read_budget_exceeded"
            };
            let error = RuntimeHostError::request(
                code,
                "read_runtime_material",
                RuntimeErrorCode::InvalidRequest,
            );
            self.record_material_reply_failure(request, &receipt, &error)?;
            receipt
                .fail_material_read(
                    RuntimeMaterialReadState::NotProvided,
                    Some(limit),
                    material_failure(&error),
                )
                .map_err(|_| receipt_error())?;
            body = self.encode_material_reply(request, &receipt)?;
        }
        if body.len() > context.max_reply_bytes {
            let error = RuntimeHostError::request(
                "material_read_reply_limit",
                "read_runtime_material",
                RuntimeErrorCode::ProtocolInvalid,
            );
            self.record_material_reply_failure(request, &receipt, &error)?;
            return Err(error);
        }
        Ok((receipt, body))
    }

    fn encode_material_reply(
        &self,
        request: &RuntimeRequest,
        receipt: &RuntimeReceipt,
    ) -> RuntimeHostResult<Vec<u8>> {
        match serde_json::to_vec(receipt) {
            Ok(body) => Ok(body),
            Err(_) => {
                let error = protocol_error("material_read_reply_encode_failed");
                self.record_material_reply_failure(request, receipt, &error)?;
                Err(error)
            }
        }
    }

    fn record_material_reply_failure(
        &self,
        request: &RuntimeRequest,
        receipt: &RuntimeReceipt,
        error: &RuntimeHostError,
    ) -> RuntimeHostResult<()> {
        if let (Ok(validated), RuntimeOperation::ReadMaterial { request: selection }) =
            (request.validate(), request.operation())
        {
            let resolved = matches!(receipt.result(), Some(RuntimeResult::MaterialRead { result }) if result.source.is_some());
            self.record_material_read_failure(&validated, error, selection, resolved)?;
        }
        Ok(())
    }
}

fn material_selection(request: &RuntimeMaterialReadRequest) -> LedgerArtifactSelection {
    LedgerArtifactSelection {
        event: request.event,
        artifact_id: request.artifact_id,
        snapshot_position: request.snapshot_position,
        sha256: request.sha256.clone(),
        byte_count: request.byte_count,
        run_id: request.expected_run_id,
        frame_id: request.expected_frame_id,
        request_id: request.expected_request_id,
        correlation_id: request.expected_correlation_id,
    }
}

fn material_source(resolved: &ResolvedLedgerArtifact) -> RuntimeMaterialReadSource {
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

fn set_retention_result(result: &mut RuntimeMaterialReadResult) -> bool {
    let Some(eviction) = result
        .source
        .as_ref()
        .and_then(|source| source.eviction.as_ref())
    else {
        return false;
    };
    result.state = RuntimeMaterialReadState::NotProvided;
    result.limit = Some(match eviction.disposition {
        None => RuntimeMaterialReadLimit::PendingEviction,
        Some(
            ArtifactEvictionDisposition::Deleted | ArtifactEvictionDisposition::RecoveryAbsent,
        ) => RuntimeMaterialReadLimit::Evicted,
        Some(ArtifactEvictionDisposition::Failed) => RuntimeMaterialReadLimit::EvictionFailed,
    });
    true
}

fn material_failure(error: &RuntimeHostError) -> RuntimeMaterialReadFailure {
    RuntimeMaterialReadFailure {
        code: error.code().to_owned(),
        operation: error.operation().to_owned(),
        error: error.projection().clone(),
    }
}
