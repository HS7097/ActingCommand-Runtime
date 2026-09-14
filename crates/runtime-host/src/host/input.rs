// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

impl HostShared {
    fn recover_safe_reset(
        &self,
        request: &RuntimeRequest,
        instance_id: InstanceId,
    ) -> Result<Option<OperationSuccess>, RequestFailure> {
        let events = self
            .ledger
            .query(EventQuery {
                request_id: Some(request.request_id()),
                ..EventQuery::default()
            })
            .map_err(|_| {
                RequestFailure::poison_without_terminal(ledger_error("recover_safe_reset"))
            })?;
        if events.is_empty() {
            return Ok(None);
        }
        if events.iter().any(|event| {
            event.links().correlation_id() != Some(&request.correlation_id())
                || event
                    .links()
                    .instance_id()
                    .is_some_and(|actual| actual != &instance_id)
        }) {
            return Err(safe_reset_replay_denied(
                "safe_reset_request_identity_reused",
            ));
        }
        let c4_lifecycle = events.iter().any(|event| {
            matches!(
                event.event_type(),
                EventType::CliCommand | EventType::LabRequest
            )
        }) && events
            .iter()
            .any(|event| event.event_type() == EventType::CommandValidated);
        if !c4_lifecycle {
            return Err(safe_reset_replay_denied("safe_reset_request_id_reused"));
        }
        let committed = events
            .iter()
            .filter(|event| {
                event.event_type() == EventType::InputCommitted
                    && matches!(
                        event.payload(),
                        EventPayload::Input(InputPayload::Committed(detail))
                            if detail.action() == EventAction::InputReset
                    )
            })
            .collect::<Vec<_>>();
        let released = events
            .iter()
            .filter(|event| event.event_type() == EventType::LeaseReleased)
            .collect::<Vec<_>>();
        match (committed.as_slice(), released.as_slice()) {
            ([input], [release]) if input.sequence() < release.sequence() => {
                let action_id = input.links().action_id().copied().ok_or_else(|| {
                    RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                        "safe_reset_action_id_missing",
                        "recover_safe_reset",
                        RuntimeErrorCode::RuntimeFatal,
                    ))
                })?;
                Ok(Some(OperationSuccess {
                    state: RuntimeReceiptState::Completed,
                    terminal: Some(terminal(release)),
                    result: RuntimeResult::SafeResetCompleted { action_id },
                }))
            }
            ([], []) => Err(safe_reset_replay_denied(
                "safe_reset_previous_attempt_incomplete",
            )),
            _ => Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "safe_reset_durable_state_inconsistent",
                    "recover_safe_reset",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            )),
        }
    }

    pub(super) fn safe_reset(
        &self,
        original: &RuntimeRequest,
        request: &ValidatedRuntimeRequest<'_>,
        instance_alias: &str,
        holder_id: actingcommand_contract::HolderId,
        connection_id: ConnectionId,
    ) -> Result<OperationSuccess, RequestFailure> {
        let resolved = self.resolve_instance(instance_alias)?;
        if let Some(recovered) = self.recover_safe_reset(original, resolved.instance_id())? {
            return Ok(recovered);
        }
        self.append_request_lifecycle(
            original,
            request,
            resolved.instance_id(),
            EventAction::InputReset,
            None,
        )?;
        let acquired = self.acquire_lease(RuntimeLeaseAcquisition {
            request,
            request_id: original.request_id(),
            instance_alias,
            holder_id,
            connection_id,
            run_links: None,
            lease_ttl_ms: None,
        })?;
        let RuntimeResult::LeaseGranted { token } = acquired.result else {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "safe_reset_lease_result_invalid",
                    "execute_safe_reset",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            ));
        };
        let action = match self.input(
            request,
            &token,
            &InputAction::Reset,
            connection_id,
            ExecutionBackendProvenance::PhysicalDevice,
            RuntimeInputContext::default(),
        ) {
            Ok((success, _)) => success,
            Err(failure) => {
                return Err(self.cleanup_composite_failure(token, connection_id, failure));
            }
        };
        let RuntimeResult::InputCommitted { action_id } = action.result else {
            return Err(self.cleanup_composite_failure(
                token,
                connection_id,
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    "safe_reset_input_result_invalid",
                    "execute_safe_reset",
                    RuntimeErrorCode::RuntimeFatal,
                )),
            ));
        };
        let released =
            match self.release_lease(request, original.request_id(), &token, connection_id, None) {
                Ok(success) => success,
                Err(failure) => {
                    return Err(self.cleanup_composite_failure(token, connection_id, failure));
                }
            };
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: released.terminal,
            result: RuntimeResult::SafeResetCompleted { action_id },
        })
    }

    fn recover_application_lifecycle(
        &self,
        request: &RuntimeRequest,
        instance_id: InstanceId,
        action: ApplicationLifecycleAction,
    ) -> Result<Option<OperationSuccess>, RequestFailure> {
        let events = self
            .ledger
            .query(EventQuery {
                request_id: Some(request.request_id()),
                ..EventQuery::default()
            })
            .map_err(|_| {
                RequestFailure::poison_without_terminal(ledger_error(
                    "recover_application_lifecycle",
                ))
            })?;
        if events.is_empty() {
            return Ok(None);
        }
        if events.iter().any(|event| {
            event.links().correlation_id() != Some(&request.correlation_id())
                || event
                    .links()
                    .instance_id()
                    .is_some_and(|actual| actual != &instance_id)
        }) {
            return Err(application_replay_denied(
                "application_lifecycle_request_identity_reused",
            ));
        }
        let expected_action = action.event_action();
        let completed = events
            .iter()
            .filter(|event| {
                matches!(
                    event.payload(),
                    EventPayload::Application(ApplicationPayload::Completed(detail))
                        if detail.action() == expected_action
                )
            })
            .collect::<Vec<_>>();
        let released = events
            .iter()
            .filter(|event| event.event_type() == EventType::LeaseReleased)
            .collect::<Vec<_>>();
        match (completed.as_slice(), released.as_slice()) {
            ([application], [release]) if application.sequence() < release.sequence() => {
                let action_id = application.links().action_id().copied().ok_or_else(|| {
                    RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                        "application_lifecycle_action_id_missing",
                        "recover_application_lifecycle",
                        RuntimeErrorCode::RuntimeFatal,
                    ))
                })?;
                Ok(Some(OperationSuccess {
                    state: RuntimeReceiptState::Completed,
                    terminal: Some(terminal(release)),
                    result: RuntimeResult::ApplicationLifecycleCompleted { action_id, action },
                }))
            }
            ([], []) => Err(application_replay_denied(
                "application_lifecycle_previous_attempt_incomplete",
            )),
            _ => Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "application_lifecycle_durable_state_inconsistent",
                    "recover_application_lifecycle",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            )),
        }
    }

    pub(super) fn application_lifecycle(
        &self,
        original: &RuntimeRequest,
        request: &ValidatedRuntimeRequest<'_>,
        instance_alias: &str,
        holder_id: actingcommand_contract::HolderId,
        action: ApplicationLifecycleAction,
        connection_id: ConnectionId,
    ) -> Result<OperationSuccess, RequestFailure> {
        let resolved = self.resolve_instance(instance_alias)?;
        if let Some(recovered) =
            self.recover_application_lifecycle(original, resolved.instance_id(), action)?
        {
            return Ok(recovered);
        }
        self.append_request_lifecycle(
            original,
            request,
            resolved.instance_id(),
            action.event_action(),
            None,
        )?;
        let acquired = self.acquire_lease(RuntimeLeaseAcquisition {
            request,
            request_id: original.request_id(),
            instance_alias,
            holder_id,
            connection_id,
            run_links: None,
            lease_ttl_ms: None,
        })?;
        let RuntimeResult::LeaseGranted { token } = acquired.result else {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "application_lifecycle_lease_result_invalid",
                    "execute_application_lifecycle",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            ));
        };
        let executed = match self.application_control(request, &token, action, connection_id) {
            Ok(success) => success,
            Err(failure) => {
                return Err(self.cleanup_composite_failure(token, connection_id, failure));
            }
        };
        let RuntimeResult::ApplicationLifecycleCompleted { action_id, .. } = executed.result else {
            return Err(self.cleanup_composite_failure(
                token,
                connection_id,
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    "application_lifecycle_result_invalid",
                    "execute_application_lifecycle",
                    RuntimeErrorCode::RuntimeFatal,
                )),
            ));
        };
        let released =
            match self.release_lease(request, original.request_id(), &token, connection_id, None) {
                Ok(success) => success,
                Err(failure) => {
                    return Err(self.cleanup_composite_failure(token, connection_id, failure));
                }
            };
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: released.terminal,
            result: RuntimeResult::ApplicationLifecycleCompleted { action_id, action },
        })
    }
}

fn safe_reset_replay_denied(code: &'static str) -> RequestFailure {
    RequestFailure::request(
        RuntimeHostError::request(
            code,
            "recover_safe_reset",
            RuntimeErrorCode::ProtocolInvalid,
        ),
        RuntimeReceiptState::Denied,
        None,
    )
}

fn application_replay_denied(code: &'static str) -> RequestFailure {
    RequestFailure::request(
        RuntimeHostError::request(
            code,
            "recover_application_lifecycle",
            RuntimeErrorCode::ProtocolInvalid,
        ),
        RuntimeReceiptState::Denied,
        None,
    )
}
