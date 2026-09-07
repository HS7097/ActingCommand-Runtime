// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_artifact_store::read_projected_verified;
use actingcommand_contract::{
    ContainedLabOperationRequest, ContainedLabOperationResult, LabOperationEvidence,
    MAX_INPUT_DURATION_MS, verify_lab_operation_evidence,
};

/// Ordinary operation failure is retained in the verified receipt and record.
pub struct VerifiedLabOperation {
    receipt: RuntimeReceipt,
    operation: ContainedLabOperationResult,
}

impl VerifiedLabOperation {
    pub fn receipt(&self) -> &RuntimeReceipt {
        &self.receipt
    }
    pub fn operation(&self) -> &ContainedLabOperationResult {
        &self.operation
    }
}

impl fmt::Debug for VerifiedLabOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("VerifiedLabOperation(<verified-artifacts>)")
    }
}

impl RuntimeDebugSession {
    pub fn run_contained_lab_operation(
        &self,
        instance_alias: &str,
        request: ContainedLabOperationRequest,
    ) -> RuntimeClientResult<VerifiedLabOperation> {
        request
            .validate()
            .map_err(|_| lab_error("runtime_lab_operation_request_invalid"))?;
        let registry = self.client.status()?;
        let instance = registry
            .instances()
            .iter()
            .find(|instance| instance.instance_alias() == instance_alias)
            .ok_or_else(|| lab_error("runtime_lab_operation_instance_unknown"))?
            .instance_id();
        let connection = self.client.connection("run_contained_lab_operation")?;
        let holder = connection
            .ids
            .mint_holder_id()
            .map_err(|_| lab_error("runtime_lab_holder_issue_failed"))?;
        let timeout = connection
            .backend_open_timeout
            .checked_mul(4)
            .and_then(|timeout| timeout.checked_add(Duration::from_millis(MAX_INPUT_DURATION_MS)))
            .and_then(|timeout| timeout.checked_add(connection.io_timeout))
            .ok_or_else(|| lab_error("runtime_lab_operation_timeout_overflow"))?;
        drop(connection);
        let receipt = self.client.execute_receipt_with_correlation(
            "run_contained_lab_operation",
            RuntimeOperation::RunContainedLabOperation {
                instance_alias: instance_alias.to_string(),
                holder_id: *holder.transport(),
                request: request.clone(),
            },
            self.correlation,
            Some(timeout),
        )?;
        let verification = (|| {
            let Some(RuntimeResult::ContainedLabOperation { operation }) = receipt.result() else {
                return Err(lab_error("runtime_lab_operation_result_unexpected"));
            };
            let prepared = &operation.record.prepared;
            if prepared.instance_id != instance
                || prepared.expected_package_sha256 != request.expected_sha256
                || prepared.selection != request.selection
                || prepared.projection_hint != request.projection_hint
            {
                return Err(lab_error("runtime_lab_operation_request_mismatch"));
            }
            let events = self.client.query_events(
                EventQuery {
                    request_id: Some(receipt.request_id()),
                    correlation_id: Some(self.correlation_id()),
                    ..EventQuery::default()
                },
                ProjectionProfile::Forensic,
            )?;
            let lease_events = if let Some(lease) = prepared.lease_id {
                self.client.query_events(
                    EventQuery {
                        lease_id: Some(lease),
                        ..EventQuery::default()
                    },
                    ProjectionProfile::Forensic,
                )?
            } else {
                Vec::new()
            };
            let action_events = if let Some(action_id) = operation.record.input_action_id {
                self.client.query_events(
                    EventQuery {
                        action_id: Some(action_id),
                        ..EventQuery::default()
                    },
                    ProjectionProfile::Forensic,
                )?
            } else {
                Vec::new()
            };
            verify_lab_operation(
                &self.client.shared.state_root,
                &receipt,
                &events,
                &lease_events,
                &action_events,
            )?;
            Ok(VerifiedLabOperation {
                operation: operation.as_ref().clone(),
                receipt: receipt.clone(),
            })
        })();
        verification.map_err(
            |error| match self.client.connection("verify_lab_operation") {
                Ok(mut connection) => connection.latch(error),
                Err(error) => error,
            },
        )
    }
}

fn lab_error(code: &'static str) -> RuntimeClientError {
    RuntimeClientError::fatal(code, "verify_contained_lab_operation")
}

fn verify_lab_operation(
    root: &Path,
    receipt: &RuntimeReceipt,
    events: &[ProjectedEvent],
    lease_events: &[ProjectedEvent],
    action_events: &[ProjectedEvent],
) -> RuntimeClientResult<()> {
    receipt
        .validate()
        .map_err(|_| lab_error("runtime_lab_receipt_invalid"))?;
    let Some(RuntimeResult::ContainedLabOperation { operation }) = receipt.result() else {
        return Err(lab_error("runtime_lab_result_missing"));
    };
    let mut artifacts = std::collections::BTreeMap::new();
    for reference in events
        .iter()
        .filter(|event| event.event_type == EventType::ArtifactVerified)
        .flat_map(|event| &event.artifacts)
    {
        let bytes = read_projected_verified(root, reference)
            .map_err(|_| lab_error("runtime_lab_artifact_hash_mismatch"))?;
        if artifacts.insert(reference.artifact_id, bytes).is_some() {
            return Err(lab_error("runtime_lab_artifact_event_duplicate"));
        }
    }
    let evidence = LabOperationEvidence {
        operation: operation.as_ref().clone(),
        terminal: receipt
            .terminal()
            .ok_or_else(|| lab_error("runtime_lab_terminal_missing"))?,
        events: events.to_vec(),
        lease_events: lease_events.to_vec(),
        action_events: action_events.to_vec(),
        artifacts,
    };
    verify_lab_operation_evidence(&evidence).map_err(|error| lab_error(error.code()))
}
