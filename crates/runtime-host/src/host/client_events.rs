// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::codes::{HostCode, HostLocation};
use actingcommand_contract::codes::{ContractLocation, RuntimeErrorCode, SharedCode};

impl HostShared {
    pub(super) fn release_lab_pin(
        &self,
        validated: &ValidatedRuntimeRequest<'_>,
        target: actingcommand_contract::LabPinReleaseTarget,
    ) -> Result<OperationSuccess, RequestFailure> {
        let requested = self.append_event(
            EventSeverity::Info,
            EventSource::Lab,
            OriginModule::Actinglab,
            EventActor::Lab,
            validated.event_links(None, None, None),
            ClientPayloadDraft::lab_pin_release(target, AuditInput::new()),
        )?;
        let release = self
            .ledger
            .release_lab_pin(target, terminal(&requested))
            .map_err(|error| {
                if error.is_fatal() {
                    RequestFailure::poison_without_terminal(
                        RuntimeHostError::relayed_fatal(
                            error.code(),
                            ContractLocation::ReleaseLabPin.as_str(),
                            RuntimeErrorCode::LedgerFailure,
                        )
                        .with_native_detail(error.to_string()),
                    )
                } else {
                    RequestFailure::request(
                        RuntimeHostError::relayed_request(
                            error.code(),
                            ContractLocation::ReleaseLabPin.as_str(),
                            RuntimeErrorCode::InvalidRequest,
                        ),
                        RuntimeReceiptState::Denied,
                        Some(terminal(&requested)),
                    )
                }
            })?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(release.released),
            result: RuntimeResult::LabPinReleased {
                release: Box::new(release),
            },
            failure_outcome: None,
        })
    }

    pub(super) fn record_authoring_event(
        &self,
        validated: &ValidatedRuntimeRequest<'_>,
        event: &ResourceAuthoringEvent,
    ) -> Result<OperationSuccess, RequestFailure> {
        let severity = if event.phase() == ResourceAuthoringPhase::PromoteFailed {
            EventSeverity::Error
        } else {
            EventSeverity::Info
        };
        let persisted = self.append_event(
            severity,
            EventSource::Lab,
            OriginModule::ResourceTooling,
            EventActor::Lab,
            validated.event_links(None, None, None),
            ResourceAuthoringPayloadDraft::event(
                event.phase(),
                event.draft_id(),
                event.target_label(),
                event.target_fingerprint(),
                event.changed_paths().to_vec(),
                event.outcome(),
                AuditInput::new(),
            ),
        )?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(terminal(&persisted)),
            result: RuntimeResult::AuthoringEventRecorded {
                phase: event.phase(),
            },
            failure_outcome: None,
        })
    }

    pub(super) fn record_debug_event(
        &self,
        validated: &ValidatedRuntimeRequest<'_>,
        event: &RuntimeDebugEvent,
    ) -> Result<OperationSuccess, RequestFailure> {
        let context = lock(&self.debug_runs, HostLocation::ReadRuntimeDebugEventContext)?
            .get(&validated.correlation_id())
            .cloned();
        if event.operation() == RuntimeDebugOperation::LabRun && context.is_none() {
            return Err(RequestFailure::request(
                RuntimeHostError::request(
                    HostCode::RuntimeDebugContextMissing,
                    ContractLocation::RecordRuntimeDebugEvent,
                    RuntimeErrorCode::InvalidRequest,
                ),
                RuntimeReceiptState::Denied,
                None,
            ));
        }
        let links = context.as_ref().map_or_else(
            || validated.event_links(None, None, None),
            |context| validated.task_event_links(context.task_id, context.run_id),
        );
        let action = event.operation().event_action();
        let persisted = match (event.operation(), event.phase()) {
            (_, RuntimeDebugPhase::Requested) => self.append_event(
                EventSeverity::Info,
                EventSource::Lab,
                OriginModule::Actinglab,
                EventActor::Lab,
                links,
                ClientPayloadDraft::lab_request(action, AuditInput::new()),
            )?,
            (RuntimeDebugOperation::LabRun, RuntimeDebugPhase::Progress) => self.append_event(
                EventSeverity::Info,
                EventSource::Lab,
                OriginModule::Actinglab,
                EventActor::Lab,
                links,
                TaskPayloadDraft::step_finished(action, AuditInput::new()),
            )?,
            (RuntimeDebugOperation::LabRun, RuntimeDebugPhase::Completed) => self.append_event(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
                links,
                TaskPayloadDraft::completed(action, event.effect_disposition(), AuditInput::new()),
            )?,
            (RuntimeDebugOperation::LabRun, RuntimeDebugPhase::Failed) => self.append_event(
                EventSeverity::Error,
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
                links,
                TaskPayloadDraft::failed(
                    action,
                    DiagnosticCode::RuntimeDiagnostic,
                    event.effect_disposition(),
                    AuditInput::new(),
                ),
            )?,
            (_, RuntimeDebugPhase::Completed) => self.append_event(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
                links,
                CommandPayloadDraft::validated(
                    action,
                    event.effect_disposition(),
                    AuditInput::new(),
                ),
            )?,
            (_, RuntimeDebugPhase::Failed) => self.append_event(
                EventSeverity::Error,
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
                links,
                CommandPayloadDraft::rejected(
                    action,
                    DiagnosticCode::CommandRejected,
                    event.effect_disposition(),
                    AuditInput::new(),
                ),
            )?,
            (_, RuntimeDebugPhase::Progress) => {
                return Err(RequestFailure::request(
                    RuntimeHostError::request(
                        SharedCode::RuntimeDebugEventInvalid,
                        ContractLocation::RecordRuntimeDebugEvent,
                        RuntimeErrorCode::InvalidRequest,
                    ),
                    RuntimeReceiptState::Denied,
                    None,
                ));
            }
        };
        if event.operation() == RuntimeDebugOperation::LabRun {
            let outcome = match event.phase() {
                RuntimeDebugPhase::Completed => Some(TaskOutcome::Success),
                RuntimeDebugPhase::Failed => Some(TaskOutcome::Failure),
                _ => None,
            };
            if let Some(outcome) = outcome {
                let mut debug_runs =
                    lock(&self.debug_runs, HostLocation::UpdateRuntimeDebugRunOutcome)?;
                let context = debug_runs
                    .get_mut(&validated.correlation_id())
                    .ok_or_else(|| {
                        RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                            HostCode::RuntimeDebugContextMissingAfterTerminal,
                            ContractLocation::RecordRuntimeDebugEvent,
                            RuntimeErrorCode::RuntimeHalted,
                        ))
                    })?;
                context.terminal_outcome = Some(outcome);
            }
        }
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(terminal(&persisted)),
            result: RuntimeResult::DebugEventRecorded {
                phase: event.phase(),
            },
            failure_outcome: None,
        })
    }

    pub(super) fn record_client_action(
        &self,
        request: &RuntimeRequest,
        validated: &ValidatedRuntimeRequest<'_>,
        action: &ClientActionRecord,
    ) -> Result<OperationSuccess, RequestFailure> {
        let _gate = lock(
            &self.governance_write_gate,
            ContractLocation::RecordClientAction,
        )
        .map_err(RequestFailure::poison_without_terminal)?;
        if let Some(existing) = self.client_fact_replay(request, EventType::ClientAction)? {
            let EventPayload::Client(ClientPayload::Action(payload)) = existing.payload() else {
                return Err(RequestFailure::poison_without_terminal(
                    RuntimeHostError::fatal(
                        HostCode::ClientActionReplayPayloadMismatch,
                        ContractLocation::RecordClientAction,
                        RuntimeErrorCode::RuntimeHalted,
                    ),
                ));
            };
            if payload.record() != action {
                return Err(client_fact_conflict(
                    HostCode::ClientActionReplayConflict,
                    ContractLocation::RecordClientAction,
                ));
            }
            return Ok(OperationSuccess {
                state: RuntimeReceiptState::Completed,
                terminal: Some(terminal(&existing)),
                result: RuntimeResult::ClientActionRecorded,
                failure_outcome: None,
            });
        }
        let instance_id = match action.instance_alias() {
            Some(alias) => Some(self.registered_instance_id(alias)?),
            None => None,
        };
        let persisted = self.append_event(
            EventSeverity::Info,
            request.source(),
            OriginModule::Governance,
            request.actor(),
            validated.event_links(instance_id, None, None),
            ClientPayloadDraft::action(action.clone(), AuditInput::new()),
        )?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(terminal(&persisted)),
            result: RuntimeResult::ClientActionRecorded,
            failure_outcome: None,
        })
    }

    pub(super) fn client_fact_replay(
        &self,
        request: &RuntimeRequest,
        event_type: EventType,
    ) -> Result<Option<PersistedEvent>, RequestFailure> {
        let mut events = self
            .ledger
            .query(EventQuery {
                request_id: Some(request.request_id()),
                ..EventQuery::default()
            })
            .map_err(|_| {
                RequestFailure::poison(ledger_error(HostLocation::QueryClientFactReplay), None)
            })?;
        if events.len() > 1 {
            if events.iter().all(|event| {
                event.event_type() == event_type
                    && event.origin().module() == OriginModule::Governance
            }) {
                return Err(RequestFailure::poison_without_terminal(
                    RuntimeHostError::fatal(
                        HostCode::ClientFactReplayAmbiguous,
                        HostLocation::QueryClientFactReplay,
                        RuntimeErrorCode::RuntimeHalted,
                    ),
                ));
            }
            return Err(client_fact_conflict(
                HostCode::ClientFactRequestIdConflict,
                HostLocation::QueryClientFactReplay,
            ));
        }
        let Some(event) = events.pop() else {
            return Ok(None);
        };
        if event.event_type() != event_type
            || event.origin().source() != request.source()
            || event.origin().actor() != request.actor()
            || event.origin().module() != OriginModule::Governance
            || event.links().correlation_id() != Some(&request.correlation_id())
        {
            return Err(client_fact_conflict(
                HostCode::ClientFactReplayOriginConflict,
                HostLocation::QueryClientFactReplay,
            ));
        }
        Ok(Some(event))
    }

    fn registered_instance_id(&self, instance_alias: &str) -> Result<InstanceId, RequestFailure> {
        lock(
            &self.registered_instances,
            HostLocation::ResolveClientActionInstance,
        )?
        .values()
        .find(|instance| instance.instance_alias == instance_alias)
        .map(RegisteredInstance::instance_id)
        .ok_or_else(|| {
            RequestFailure::request(
                RuntimeHostError::request(
                    RuntimeErrorCode::InstanceUnknown,
                    HostLocation::ResolveClientActionInstance,
                    RuntimeErrorCode::InstanceUnknown,
                ),
                RuntimeReceiptState::Denied,
                None,
            )
        })
    }
}
