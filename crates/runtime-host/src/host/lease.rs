// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[derive(Clone)]
pub(super) struct QueuedRequestContext {
    request: RuntimeRequest,
    instance: RegisteredInstance,
    connection_id: ConnectionId,
}

#[derive(Clone, Copy)]
struct QueueTerminalRecord {
    connection_id: ConnectionId,
    terminal: TerminalEvent,
    outcome: QueueTerminalOutcome,
}

#[derive(Clone, Copy)]
enum QueueTerminalOutcome {
    Expired,
    Cancelled { instance_id: InstanceId },
}

/// Bounded process-local replay cache; durable terminal history remains in the ledger.
#[derive(Default)]
pub(super) struct QueueTerminalStore {
    entries: BTreeMap<RequestId, QueueTerminalRecord>,
    order: VecDeque<RequestId>,
}

impl QueueTerminalStore {
    fn insert(&mut self, request_id: RequestId, record: QueueTerminalRecord) {
        if self.entries.insert(request_id, record).is_none() {
            self.order.push_back(request_id);
        }
        while self.order.len() > MAX_REQUEST_CACHE_ENTRIES {
            if let Some(expired) = self.order.pop_front() {
                self.entries.remove(&expired);
            }
        }
    }
}

impl HostShared {
    pub(super) fn queue_lease(
        &self,
        original: &RuntimeRequest,
        request: &ValidatedRuntimeRequest<'_>,
        instance_alias: &str,
        holder_id: actingcommand_contract::HolderId,
        policy: LeaseQueuePolicy,
        connection_id: ConnectionId,
    ) -> Result<OperationSuccess, RequestFailure> {
        let resolved = self.resolve_instance(instance_alias)?;
        let instance_guard = self.instance_guard(resolved.instance_id())?;
        let admission = lock(&instance_guard, "lock_instance_admission")?;
        self.expire_instance_if_due(resolved.instance_id())?;
        let outcome = lock(&self.scheduler, "queue_lease")?.request_queued(
            QueueLeaseRequest::new(
                original.request_id(),
                resolved.instance_id(),
                holder_id,
                connection_id,
                policy.priority(),
                policy.timeout_ms(),
            ),
            self.monotonic_ms()?,
        );
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                self.append_lease_requested(request, &resolved)?;
                return Err(self.scheduler_denied(request, &resolved, None, error)?);
            }
        };
        let (decision, expired) = outcome.into_parts();
        self.record_expired_queued(expired)?;
        match decision {
            QueueAdmissionDecision::Lease(preparation) if preparation.is_existing() => {
                let token = preparation.token().clone();
                let terminal =
                    self.existing_queue_grant_terminal(original.request_id(), token.lease_id())?;
                Ok(OperationSuccess {
                    state: RuntimeReceiptState::Admitted,
                    terminal: Some(terminal),
                    result: RuntimeResult::LeaseGranted { token },
                })
            }
            QueueAdmissionDecision::Lease(preparation) => self.grant_prepared_lease(
                request,
                original.request_id(),
                &resolved,
                preparation,
                None,
            ),
            QueueAdmissionDecision::Queued(queued) => {
                if let Some(existing) = lock(&self.queued_requests, "read_queued_request")?
                    .get(&original.request_id())
                    .cloned()
                {
                    if existing.request != *original
                        || existing.instance != resolved
                        || existing.connection_id != connection_id
                    {
                        return Err(RequestFailure::poison_without_terminal(
                            RuntimeHostError::fatal(
                                "queued_request_identity_mismatch",
                                "queue_lease",
                                RuntimeErrorCode::RuntimeFatal,
                            ),
                        ));
                    }
                    let terminal = self.existing_request_terminal(
                        original.request_id(),
                        EventType::SchedulerQueued,
                    )?;
                    return Ok(OperationSuccess {
                        state: RuntimeReceiptState::Queued,
                        terminal: Some(terminal),
                        result: RuntimeResult::LeaseQueued {
                            status: queued.status().map_err(|error| {
                                RequestFailure::poison_without_terminal(
                                    RuntimeHostError::scheduler("queue_lease_status", &error),
                                )
                            })?,
                        },
                    });
                }
                self.append_lease_requested(request, &resolved)?;
                let mut terminal_event =
                    self.append_scheduler_queued(request, &resolved, &queued)?;
                if queued.preempt_requested() {
                    terminal_event =
                        self.append_scheduler_preempted(request, &resolved, &queued)?;
                }
                let context = QueuedRequestContext {
                    request: original.clone(),
                    instance: resolved,
                    connection_id,
                };
                if lock(&self.queued_requests, "register_queued_request")?
                    .insert(original.request_id(), context)
                    .is_some()
                {
                    return Err(RequestFailure::poison_without_terminal(
                        RuntimeHostError::fatal(
                            "queued_request_collision",
                            "queue_lease",
                            RuntimeErrorCode::RuntimeFatal,
                        ),
                    ));
                }
                if let Some((token, transferred)) =
                    self.promote_idle_preemption(&queued, &admission)?
                {
                    return Ok(OperationSuccess {
                        state: RuntimeReceiptState::Admitted,
                        terminal: Some(terminal(&transferred)),
                        result: RuntimeResult::LeaseGranted { token },
                    });
                }
                Ok(OperationSuccess {
                    state: RuntimeReceiptState::Queued,
                    terminal: Some(terminal(&terminal_event)),
                    result: RuntimeResult::LeaseQueued {
                        status: queued.status().map_err(|error| {
                            RequestFailure::poison_without_terminal(RuntimeHostError::scheduler(
                                "queue_lease_status",
                                &error,
                            ))
                        })?,
                    },
                })
            }
        }
    }

    #[cfg(test)]
    fn wait_queue_operation_test_hook(
        &self,
        operation: QueueOperationTestKind,
        request_id: RequestId,
    ) -> Result<(), RequestFailure> {
        let hook = {
            let mut slot = lock(
                &self.queue_operation_test_hook,
                "read_queue_operation_test_hook",
            )?;
            slot.as_ref()
                .is_some_and(|hook| hook.operation == operation && hook.request_id == request_id)
                .then(|| slot.take())
                .flatten()
        };
        if let Some(hook) = hook {
            hook.snapshot_reached.wait();
            hook.resume.wait();
        }
        Ok(())
    }

    fn existing_queue_terminal_result(
        &self,
        queued_request_id: RequestId,
        connection_id: ConnectionId,
        operation: &'static str,
    ) -> Result<Option<OperationSuccess>, RequestFailure> {
        let record = lock(&self.queue_terminals, "read_queue_terminal")?
            .entries
            .get(&queued_request_id)
            .copied();
        let Some(record) = record else {
            return Ok(None);
        };
        if record.connection_id != connection_id {
            return Err(RequestFailure::request(
                RuntimeHostError::request(
                    "lease_queue_connection_mismatch",
                    operation,
                    RuntimeErrorCode::QueueConnectionMismatch,
                ),
                RuntimeReceiptState::Denied,
                None,
            ));
        }
        match record.outcome {
            QueueTerminalOutcome::Expired => Err(RequestFailure::request(
                RuntimeHostError::request(
                    "lease_queue_expired",
                    operation,
                    RuntimeErrorCode::QueueExpired,
                ),
                RuntimeReceiptState::Denied,
                Some(record.terminal),
            )),
            QueueTerminalOutcome::Cancelled { instance_id } => Ok(Some(OperationSuccess {
                state: RuntimeReceiptState::Cancelled,
                terminal: Some(record.terminal),
                result: RuntimeResult::LeaseQueueCancelled {
                    request_id: queued_request_id,
                    instance_id,
                },
            })),
        }
    }

    pub(super) fn poll_queued_lease(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        queued_request_id: RequestId,
        connection_id: ConnectionId,
    ) -> Result<OperationSuccess, RequestFailure> {
        let context = lock(&self.queued_requests, "read_queued_request")?
            .get(&queued_request_id)
            .cloned();
        #[cfg(test)]
        self.wait_queue_operation_test_hook(QueueOperationTestKind::Poll, queued_request_id)?;
        if context.is_none()
            && let Some(success) = self.existing_queue_terminal_result(
                queued_request_id,
                connection_id,
                "poll_queued_lease",
            )?
        {
            return Ok(success);
        }
        let instance_guard = context
            .as_ref()
            .map(|context| self.instance_guard(context.instance.instance_id()))
            .transpose()?;
        let _admission = instance_guard
            .as_ref()
            .map(|guard| lock(guard, "lock_instance_admission"))
            .transpose()?;
        let poll = lock(&self.scheduler, "poll_queued_lease")?.poll_queued(
            queued_request_id,
            connection_id,
            self.monotonic_ms()?,
        );
        match poll {
            Ok(QueuePoll::Granted(token)) => {
                let terminal =
                    self.existing_queue_grant_terminal(queued_request_id, token.lease_id())?;
                Ok(OperationSuccess {
                    state: RuntimeReceiptState::Admitted,
                    terminal: Some(terminal),
                    result: RuntimeResult::LeaseGranted { token },
                })
            }
            Ok(QueuePoll::Pending(queued)) => Ok(OperationSuccess {
                state: RuntimeReceiptState::Queued,
                terminal: None,
                result: RuntimeResult::LeasePending {
                    status: queued.status().map_err(|error| {
                        RequestFailure::poison_without_terminal(RuntimeHostError::scheduler(
                            "poll_queued_lease",
                            &error,
                        ))
                    })?,
                },
            }),
            Err(SchedulerError::QueueExpired) => {
                let context = context.ok_or_else(|| {
                    RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                        "expired_queue_context_missing",
                        "poll_queued_lease",
                        RuntimeErrorCode::RuntimeFatal,
                    ))
                })?;
                let event = self.finish_queue_expiry(&context)?;
                Err(RequestFailure::request(
                    RuntimeHostError::scheduler("poll_queued_lease", &SchedulerError::QueueExpired),
                    RuntimeReceiptState::Denied,
                    Some(terminal(&event)),
                ))
            }
            Err(SchedulerError::QueueMissing) => {
                if let Some(success) = self.existing_queue_terminal_result(
                    queued_request_id,
                    connection_id,
                    "poll_queued_lease",
                )? {
                    return Ok(success);
                }
                Err(self.scheduler_denied_error(
                    request,
                    context.as_ref().map(|value| value.instance.instance_id()),
                    None,
                    context
                        .as_ref()
                        .map_or("", |value| value.instance.audit_endpoint()),
                    RuntimeHostError::scheduler("poll_queued_lease", &SchedulerError::QueueMissing),
                )?)
            }
            Err(error) => Err(self.scheduler_denied_error(
                request,
                context.as_ref().map(|value| value.instance.instance_id()),
                None,
                context
                    .as_ref()
                    .map_or("", |value| value.instance.audit_endpoint()),
                RuntimeHostError::scheduler("poll_queued_lease", &error),
            )?),
        }
    }

    pub(super) fn cancel_queued_lease(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        queued_request_id: RequestId,
        connection_id: ConnectionId,
    ) -> Result<OperationSuccess, RequestFailure> {
        let context = lock(&self.queued_requests, "read_queued_request")?
            .get(&queued_request_id)
            .cloned();
        #[cfg(test)]
        self.wait_queue_operation_test_hook(QueueOperationTestKind::Cancel, queued_request_id)?;
        if context.is_none()
            && let Some(success) = self.existing_queue_terminal_result(
                queued_request_id,
                connection_id,
                "cancel_queued_lease",
            )?
        {
            return Ok(success);
        }
        let instance_guard = context
            .as_ref()
            .map(|context| self.instance_guard(context.instance.instance_id()))
            .transpose()?;
        let _admission = instance_guard
            .as_ref()
            .map(|guard| lock(guard, "lock_instance_admission"))
            .transpose()?;
        let cancelled = lock(&self.scheduler, "cancel_queued_lease")?
            .cancel_queued(queued_request_id, connection_id);
        let cancelled = match cancelled {
            Ok(cancelled) => cancelled,
            Err(SchedulerError::QueueMissing) => {
                if let Some(success) = self.existing_queue_terminal_result(
                    queued_request_id,
                    connection_id,
                    "cancel_queued_lease",
                )? {
                    return Ok(success);
                }
                return Err(self.scheduler_denied_error(
                    request,
                    None,
                    None,
                    "",
                    RuntimeHostError::scheduler(
                        "cancel_queued_lease",
                        &SchedulerError::QueueMissing,
                    ),
                )?);
            }
            Err(error) => {
                return Err(self.scheduler_denied_error(
                    request,
                    None,
                    None,
                    "",
                    RuntimeHostError::scheduler("cancel_queued_lease", &error),
                )?);
            }
        };
        let context = self.read_queued_context_for(cancelled.queued())?;
        let event = self.finish_queue_terminal(
            &context,
            DiagnosticCode::LeaseQueueCancelled,
            QueueTerminalOutcome::Cancelled {
                instance_id: cancelled.queued().instance_id(),
            },
        )?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Cancelled,
            terminal: Some(terminal(&event)),
            result: RuntimeResult::LeaseQueueCancelled {
                request_id: queued_request_id,
                instance_id: cancelled.queued().instance_id(),
            },
        })
    }

    fn existing_queue_grant_terminal(
        &self,
        request_id: RequestId,
        lease_id: LeaseId,
    ) -> Result<TerminalEvent, RequestFailure> {
        if let Some(terminal) =
            self.query_single_terminal(request_id, Some(lease_id), EventType::LeaseTransferred)?
        {
            return Ok(terminal);
        }
        self.query_single_terminal(request_id, Some(lease_id), EventType::LeaseGranted)?
            .ok_or_else(|| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    "queue_grant_terminal_missing",
                    "recover_queued_lease_grant",
                    RuntimeErrorCode::RuntimeFatal,
                ))
            })
    }

    fn append_scheduler_queued(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        resolved: &RegisteredInstance,
        queued: &QueuedLease,
    ) -> Result<PersistedEvent, RequestFailure> {
        let links = self
            .events
            .request_links(request, Some(resolved.instance_id()), None, None);
        self.append_event(
            EventSeverity::Info,
            EventSource::Scheduler,
            OriginModule::Scheduler,
            EventActor::Scheduler,
            links,
            SchedulerPayloadDraft::queued(
                EventAction::ScheduleAdmit,
                queued.priority(),
                queued.position(),
                queued.deadline_monotonic_ms(),
                queued.preempt_requested(),
                audit_endpoint(resolved.audit_endpoint()),
            ),
        )
    }

    fn append_scheduler_preempted(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        resolved: &RegisteredInstance,
        queued: &QueuedLease,
    ) -> Result<PersistedEvent, RequestFailure> {
        let active = lock(&self.scheduler, "read_preemption_state")?
            .active_lease(resolved.instance_id())
            .ok_or_else(|| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    "preemption_active_lease_missing",
                    "record_scheduler_preemption",
                    RuntimeErrorCode::RuntimeFatal,
                ))
            })?;
        if !active.preempt_requested() {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "preemption_state_mismatch",
                    "record_scheduler_preemption",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            ));
        }
        let links = self.events.request_links(
            request,
            Some(resolved.instance_id()),
            Some(active.token().lease_id()),
            None,
        );
        self.append_event(
            EventSeverity::Warning,
            EventSource::Scheduler,
            OriginModule::Scheduler,
            EventActor::Scheduler,
            links,
            SchedulerPayloadDraft::preempted(
                EventAction::ScheduleAdmit,
                active.token().holder_id(),
                active.token().lease_id(),
                queued.request_id(),
                queued.priority(),
                active.destructive_step_active(),
                audit_endpoint(resolved.audit_endpoint()),
            ),
        )
    }

    fn record_expired_queued(&self, expired: Vec<QueuedLease>) -> Result<(), RequestFailure> {
        for queued in expired {
            let context = self.read_queued_context_for(&queued)?;
            self.finish_queue_expiry(&context)?;
        }
        Ok(())
    }

    fn finish_queue_expiry(
        &self,
        context: &QueuedRequestContext,
    ) -> Result<PersistedEvent, RequestFailure> {
        self.finish_queue_terminal(
            context,
            DiagnosticCode::LeaseQueueExpired,
            QueueTerminalOutcome::Expired,
        )
    }

    /// Publishes the recoverable terminal before context removal, so a context miss cannot race
    /// ahead of the authoritative queue result.
    fn finish_queue_terminal(
        &self,
        context: &QueuedRequestContext,
        diagnostic: DiagnosticCode,
        outcome: QueueTerminalOutcome,
    ) -> Result<PersistedEvent, RequestFailure> {
        let event = self.append_queue_terminal(context, diagnostic)?;
        self.remember_queue_terminal(context, &event, outcome)?;
        self.remove_queued_context(context.request.request_id(), context.connection_id)?;
        Ok(event)
    }

    fn remember_queue_terminal(
        &self,
        context: &QueuedRequestContext,
        event: &PersistedEvent,
        outcome: QueueTerminalOutcome,
    ) -> Result<(), RequestFailure> {
        lock(&self.queue_terminals, "record_queue_terminal")?.insert(
            context.request.request_id(),
            QueueTerminalRecord {
                connection_id: context.connection_id,
                terminal: terminal(event),
                outcome,
            },
        );
        Ok(())
    }

    pub(super) fn cancel_instance_queue(
        &self,
        instance_id: InstanceId,
        diagnostic: DiagnosticCode,
    ) -> Result<(), RequestFailure> {
        let removed = lock(&self.scheduler, "cancel_instance_queue")?
            .remove_queued_for_instance(instance_id)
            .map_err(|error| {
                RequestFailure::poison_without_terminal(RuntimeHostError::scheduler(
                    "cancel_instance_queue",
                    &error,
                ))
            })?;
        for cancelled in removed {
            let context = self.take_queued_context(&cancelled)?;
            self.append_queue_terminal(&context, diagnostic)?;
        }
        Ok(())
    }

    pub(super) fn take_queued_context(
        &self,
        cancelled: &CancelledQueuedLease,
    ) -> Result<QueuedRequestContext, RequestFailure> {
        self.take_queued_context_for(cancelled.queued())
    }

    fn read_queued_context_for(
        &self,
        queued: &QueuedLease,
    ) -> Result<QueuedRequestContext, RequestFailure> {
        let context = lock(&self.queued_requests, "read_queued_request")?
            .get(&queued.request_id())
            .cloned()
            .ok_or_else(|| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    "queued_request_context_missing",
                    "read_queued_request",
                    RuntimeErrorCode::RuntimeFatal,
                ))
            })?;
        if context.connection_id != queued.connection_id() {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "queued_request_connection_mismatch",
                    "read_queued_request",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            ));
        }
        if context.instance.instance_id() != queued.instance_id() {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "queued_request_instance_mismatch",
                    "read_queued_request",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            ));
        }
        Ok(context)
    }

    fn take_queued_context_for(
        &self,
        queued: &QueuedLease,
    ) -> Result<QueuedRequestContext, RequestFailure> {
        let context = self.remove_queued_context(queued.request_id(), queued.connection_id())?;
        if context.instance.instance_id() != queued.instance_id() {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "queued_request_instance_mismatch",
                    "remove_queued_request",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            ));
        }
        Ok(context)
    }

    fn remove_queued_context(
        &self,
        request_id: RequestId,
        connection_id: ConnectionId,
    ) -> Result<QueuedRequestContext, RequestFailure> {
        let context = lock(&self.queued_requests, "remove_queued_request")?
            .remove(&request_id)
            .ok_or_else(|| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    "queued_request_context_missing",
                    "remove_queued_request",
                    RuntimeErrorCode::RuntimeFatal,
                ))
            })?;
        if context.connection_id != connection_id {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "queued_request_connection_mismatch",
                    "remove_queued_request",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            ));
        }
        Ok(context)
    }

    pub(super) fn append_queue_terminal(
        &self,
        context: &QueuedRequestContext,
        diagnostic: DiagnosticCode,
    ) -> Result<PersistedEvent, RequestFailure> {
        let validated = context.request.validate().map_err(|_| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                "queued_request_context_invalid",
                "record_queued_request_terminal",
                RuntimeErrorCode::RuntimeFatal,
            ))
        })?;
        let links =
            self.events
                .request_links(&validated, Some(context.instance.instance_id()), None, None);
        self.append_event(
            EventSeverity::Warning,
            EventSource::Scheduler,
            OriginModule::Scheduler,
            EventActor::Scheduler,
            links,
            SchedulerPayloadDraft::denied(
                EventAction::ScheduleAdmit,
                diagnostic,
                audit_endpoint(context.instance.audit_endpoint()),
            ),
        )
    }

    pub(super) fn expire_queued_for_instance(
        &self,
        instance_id: InstanceId,
    ) -> Result<(), RequestFailure> {
        let expired = lock(&self.scheduler, "expire_queued_requests")?
            .take_expired_for_instance(instance_id, self.monotonic_ms()?)
            .map_err(|error| {
                RequestFailure::poison_without_terminal(RuntimeHostError::scheduler(
                    "expire_queued_requests",
                    &error,
                ))
            })?;
        self.record_expired_queued(expired)
    }

    pub(super) fn perform_transfer(
        &self,
        prepared: Box<PreparedLeaseTransfer>,
    ) -> Result<PersistedEvent, RequestFailure> {
        let from = prepared.from_token().clone();
        let to = prepared.to_token().clone();
        let queued_request_id = prepared.queued_request_id();
        let to_connection_id = prepared.to_connection_id();
        let priority = prepared.priority();
        let action = match prepared.reason() {
            LeaseTransferReason::Preempted => EventAction::LeaseAcquire,
            LeaseTransferReason::Expired => EventAction::LeaseExpire,
            LeaseTransferReason::ExplicitRelease
            | LeaseTransferReason::Disconnect
            | LeaseTransferReason::BackendFailure
            | LeaseTransferReason::HostShutdown => EventAction::LeaseRelease,
        };
        let context = lock(&self.queued_requests, "read_transfer_request")?
            .get(&queued_request_id)
            .cloned()
            .ok_or_else(|| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    "lease_transfer_context_missing",
                    "prepare_lease_transfer",
                    RuntimeErrorCode::RuntimeFatal,
                ))
            })?;
        if context.connection_id != to_connection_id
            || context.instance.instance_id() != to.instance_id()
        {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "lease_transfer_context_mismatch",
                    "prepare_lease_transfer",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            ));
        }
        let validated = context.request.validate().map_err(|_| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                "lease_transfer_request_invalid",
                "prepare_lease_transfer",
                RuntimeErrorCode::RuntimeFatal,
            ))
        })?;
        let action_id = self
            .events
            .action_id()
            .map_err(RequestFailure::poison_without_terminal)?;
        let links = self.events.request_links(
            &validated,
            Some(to.instance_id()),
            Some(to.lease_id()),
            Some(action_id),
        );
        self.append_event(
            EventSeverity::Info,
            EventSource::Scheduler,
            OriginModule::Scheduler,
            EventActor::Scheduler,
            links.clone(),
            LeasePayloadDraft::transition_intent(
                action,
                audit_endpoint(context.instance.audit_endpoint()),
            ),
        )?;
        self.close_instance_resources(
            &from,
            prepared.from_connection_id(),
            EventLinksDraft::default(),
        )?;
        let transferred = self.append_event(
            EventSeverity::Info,
            EventSource::Scheduler,
            OriginModule::Scheduler,
            EventActor::Scheduler,
            links,
            LeasePayloadDraft::transferred(
                action,
                EffectDisposition::Performed,
                from.holder_id(),
                from.lease_id(),
                to.holder_id(),
                to.lease_id(),
                queued_request_id,
                priority,
                audit_endpoint(context.instance.audit_endpoint()),
            ),
        )?;
        let committed = lock(&self.scheduler, "commit_lease_transfer")?
            .commit_transfer(prepared, self.monotonic_ms()?);
        let committed = committed.map_err(|_| {
            RequestFailure::poison(
                RuntimeHostError::fatal(
                    "lease_transfer_commit_failed_after_durable_fact",
                    "commit_lease_transfer",
                    RuntimeErrorCode::RuntimeFatal,
                ),
                Some(terminal(&transferred)),
            )
        })?;
        if committed != to {
            return Err(RequestFailure::poison(
                RuntimeHostError::fatal(
                    "lease_transfer_token_mismatch_after_durable_fact",
                    "commit_lease_transfer",
                    RuntimeErrorCode::RuntimeFatal,
                ),
                Some(terminal(&transferred)),
            ));
        }
        self.remove_queued_context(queued_request_id, to_connection_id)?;
        self.persist_active_instances()
            .map_err(RequestFailure::poison_without_terminal)?;
        Ok(transferred)
    }

    /// The caller holds the per-instance admission guard, so no lease mutation can invalidate the
    /// prepared transfer before its durable authorization fact is committed.
    fn promote_idle_preemption(
        &self,
        queued: &QueuedLease,
        _admission: &MutexGuard<'_, ()>,
    ) -> Result<Option<(LeaseToken, PersistedEvent)>, RequestFailure> {
        if !queued.preempt_requested() {
            return Ok(None);
        }
        let transfer = {
            let mut scheduler = lock(&self.scheduler, "prepare_idle_preemption")?;
            let active = scheduler
                .active_lease(queued.instance_id())
                .ok_or_else(|| {
                    RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                        "idle_preemption_active_lease_missing",
                        "prepare_idle_preemption",
                        RuntimeErrorCode::RuntimeFatal,
                    ))
                })?;
            scheduler
                .prepare_transfer(
                    active.token(),
                    active.connection_id(),
                    LeaseTransferReason::Preempted,
                    None,
                    self.monotonic_ms()?,
                )
                .map_err(|error| {
                    RequestFailure::poison_without_terminal(RuntimeHostError::scheduler(
                        "prepare_idle_preemption",
                        &error,
                    ))
                })?
        };
        match transfer {
            TransferPreparation::Ready(prepared) => {
                if !self.capacity_allows_transfer(prepared.from_token())? {
                    self.cleanup_token_inner(
                        prepared.from_token(),
                        prepared.from_connection_id(),
                        LeaseReleaseReason::Preempted,
                        None,
                        Some(_admission),
                    )?;
                    return Ok(None);
                }
                let token = prepared.to_token().clone();
                self.perform_transfer(prepared)
                    .map(|event| Some((token, event)))
            }
            TransferPreparation::Deferred => Ok(None),
            TransferPreparation::NoCandidate => Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "idle_preemption_candidate_missing",
                    "prepare_idle_preemption",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            )),
        }
    }

    pub(super) fn expire_all_queued_runtime(&self) -> RuntimeHostResult<()> {
        let instance_ids = lock(&self.registered_instances, "read_instance_registry")?
            .keys()
            .copied()
            .collect::<Vec<_>>();
        for instance_id in instance_ids {
            let instance_guard = self
                .instance_guard(instance_id)
                .map_err(|failure| *failure.error)?;
            let _admission = lock(&instance_guard, "lock_instance_admission")?;
            let expired = lock(&self.scheduler, "expire_queued_requests")?
                .take_expired_for_instance(instance_id, self.monotonic_ms()?)
                .map_err(|error| RuntimeHostError::scheduler("expire_queued_requests", &error))?;
            self.record_expired_queued(expired)
                .map_err(|failure| *failure.error)?;
        }
        Ok(())
    }
}
