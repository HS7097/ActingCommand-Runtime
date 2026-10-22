// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::codes::{ClientCode, ClientLocation};
use actingcommand_artifact_store::read_projected_verified;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::{
    ContainedLabOperationRequest, ContainedLabOperationResult, LabOperationEvidence,
    LabOperationSelection, MAX_INPUT_DURATION_MS, MAX_LAB_ARRIVAL_FRAMES, ObservationFramePlan,
    verify_lab_operation_evidence,
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

/// The receipt wait of a contained Lab operation: four backend-open allowances (23 with an
/// arrival condition), the longest input, the arrival wait and the I/O allowance; #411 Lab L5:
/// and for each observation it can make (two, or 21 with an arrival condition) what its
/// transaction adds. `None` on overflow.
pub(crate) fn lab_operation_response_timeout(
    backend_open: Duration,
    io_timeout: Duration,
    request: &ContainedLabOperationRequest,
    frames: ObservationFramePlan,
) -> Option<Duration> {
    let observations = if request.after.is_some() {
        MAX_LAB_ARRIVAL_FRAMES + 1
    } else {
        2
    };
    backend_open
        .checked_mul(if request.after.is_some() { 23 } else { 4 })?
        .checked_add(Duration::from_millis(MAX_INPUT_DURATION_MS))?
        .checked_add(Duration::from_millis(
            request.after.as_ref().map_or(0, |after| after.timeout_ms),
        ))?
        .checked_add(io_timeout)?
        .checked_add(
            frames
                .added_allowance(backend_open)?
                .checked_mul(u32::try_from(observations).ok()?)?,
        )
}

/// LR-10 (#412 B-3 S2, design §2.4): the receipt wait of a Lab operation that runs a declared
/// operation: the package's task timeout (`task_timeout`, from the client's kernel admission of
/// the package it sends), four backend-open allowances, the longest input, the I/O allowance and
/// one projection transaction (what `frames` adds to the after projection). `None` on overflow.
pub(crate) fn lab_declared_operation_response_timeout(
    backend_open: Duration,
    io_timeout: Duration,
    task_timeout: Duration,
    frames: ObservationFramePlan,
) -> Option<Duration> {
    task_timeout
        .checked_add(backend_open.checked_mul(4)?)?
        .checked_add(Duration::from_millis(MAX_INPUT_DURATION_MS))?
        .checked_add(io_timeout)?
        .checked_add(frames.added_allowance(backend_open)?)
}

impl RuntimeDebugSession {
    /// `frames`: what the package's target consensus makes each of the operation's observations
    /// take (#411 Lab L5), as the kernel's `observation_frame_plan` computes it from the package
    /// sent; the receipt wait allows for it. An element or coordinate selection; a declared
    /// operation is [`Self::run_contained_lab_declared_operation`]'s.
    pub fn run_contained_lab_operation(
        &self,
        instance_alias: &str,
        request: ContainedLabOperationRequest,
        frames: ObservationFramePlan,
    ) -> RuntimeClientResult<VerifiedLabOperation> {
        self.run_lab_operation(instance_alias, request, frames, None)
    }

    /// LR-10 (#412 B-3 S2): a Lab operation that runs an operation its package declares
    /// (`LabOperationSelection::Operation`); `task_timeout` is the package's task timeout, from
    /// the client's kernel admission of the package it sends, which bounds the receipt wait
    /// (design §2.4).
    pub fn run_contained_lab_declared_operation(
        &self,
        instance_alias: &str,
        request: ContainedLabOperationRequest,
        frames: ObservationFramePlan,
        task_timeout: Duration,
    ) -> RuntimeClientResult<VerifiedLabOperation> {
        self.run_lab_operation(instance_alias, request, frames, Some(task_timeout))
    }

    /// A Lab operation; `task_timeout` is given exactly for a declared operation.
    fn run_lab_operation(
        &self,
        instance_alias: &str,
        request: ContainedLabOperationRequest,
        frames: ObservationFramePlan,
        task_timeout: Option<Duration>,
    ) -> RuntimeClientResult<VerifiedLabOperation> {
        request
            .validate()
            .map_err(|_| lab_error(ClientCode::RuntimeLabOperationRequestInvalid))?;
        if matches!(request.selection, LabOperationSelection::Operation { .. })
            != task_timeout.is_some()
        {
            return Err(lab_error(ClientCode::RuntimeLabOperationRequestInvalid));
        }
        let registry = self.client.status()?;
        let instance = registry
            .instances()
            .iter()
            .find(|instance| instance.instance_alias() == instance_alias)
            .ok_or_else(|| lab_error(ClientCode::RuntimeLabOperationInstanceUnknown))?
            .instance_id();
        let connection = self
            .client
            .connection(ClientLocation::RunContainedLabOperation.location())?;
        let holder = connection
            .ids
            .mint_holder_id()
            .map_err(|_| lab_error(ClientCode::RuntimeLabHolderIssueFailed))?;
        let timeout = match task_timeout {
            Some(task_timeout) => lab_declared_operation_response_timeout(
                connection.backend_open_timeout,
                connection.io_timeout,
                task_timeout,
                frames,
            ),
            None => lab_operation_response_timeout(
                connection.backend_open_timeout,
                connection.io_timeout,
                &request,
                frames,
            ),
        }
        .ok_or_else(|| lab_error(ClientCode::RuntimeLabOperationTimeoutOverflow))?;
        drop(connection);
        let receipt = self.client.execute_receipt_with_correlation(
            ClientLocation::RunContainedLabOperation.location(),
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
                return Err(lab_error(ClientCode::RuntimeLabOperationResultUnexpected));
            };
            let prepared = &operation.record.prepared;
            // Workflow #336 PKG-S2 (option 丙): the record names its package by `package_id`;
            // no package digest is compared.
            if prepared.instance_id != instance
                || prepared.selection != request.selection
                || prepared.projection_hint != request.projection_hint
                || prepared.after != request.after
            {
                return Err(lab_error(ClientCode::RuntimeLabOperationRequestMismatch));
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
            // LR-10 (#412 B-3): each attempt of a declared operation has its own physical action.
            let action_ids = match &operation.record.operation_result {
                Some(result) => result
                    .attempts
                    .iter()
                    .filter_map(|attempt| attempt.input_action_id)
                    .collect::<Vec<_>>(),
                None => operation.record.input_action_id.into_iter().collect(),
            };
            let mut action_events = Vec::new();
            for action_id in action_ids {
                action_events.extend(self.client.query_events(
                    EventQuery {
                        action_id: Some(action_id),
                        ..EventQuery::default()
                    },
                    ProjectionProfile::Forensic,
                )?);
            }
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
        verification.map_err(|error| {
            match self
                .client
                .connection(ClientLocation::VerifyLabOperation.location())
            {
                Ok(mut connection) => connection.latch(error),
                Err(error) => error,
            }
        })
    }
}

fn lab_error(code: impl Into<Code>) -> RuntimeClientError {
    RuntimeClientError::fatal(code, ClientLocation::VerifyContainedLabOperation.location())
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
        .map_err(|_| lab_error(ClientCode::RuntimeLabReceiptInvalid))?;
    let Some(RuntimeResult::ContainedLabOperation { operation }) = receipt.result() else {
        return Err(lab_error(ClientCode::RuntimeLabResultMissing));
    };
    let mut artifacts = std::collections::BTreeMap::new();
    for reference in events
        .iter()
        .filter(|event| event.event_type == EventType::ArtifactVerified)
        .flat_map(|event| &event.artifacts)
    {
        let bytes = read_projected_verified(root, reference)
            .map_err(|_| lab_error(SharedCode::RuntimeLabArtifactHashMismatch))?;
        if artifacts.insert(reference.artifact_id, bytes).is_some() {
            return Err(lab_error(SharedCode::RuntimeLabArtifactEventDuplicate));
        }
    }
    let evidence = LabOperationEvidence {
        operation: operation.as_ref().clone(),
        terminal: receipt
            .terminal()
            .ok_or_else(|| lab_error(ClientCode::RuntimeLabTerminalMissing))?,
        events: events.to_vec(),
        lease_events: lease_events.to_vec(),
        action_events: action_events.to_vec(),
        artifacts,
    };
    verify_lab_operation_evidence(&evidence).map_err(|error| lab_error(error.code()))
}
