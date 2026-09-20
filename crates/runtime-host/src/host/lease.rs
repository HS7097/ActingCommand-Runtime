// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[cfg(test)]
#[derive(Clone)]
pub(super) struct LeaseExpiryTestCheckpoint {
    token: LeaseToken,
    terminal: TerminalEvent,
}

#[cfg(test)]
pub(super) fn lease_token_identity_match_count(left: &LeaseToken, right: &LeaseToken) -> usize {
    [
        left.owner_epoch() == right.owner_epoch(),
        left.lease_id() == right.lease_id(),
        left.instance_id() == right.instance_id(),
        left.holder_id() == right.holder_id(),
        left.expires_at_monotonic_ms() == right.expires_at_monotonic_ms(),
    ]
    .into_iter()
    .filter(|matches| *matches)
    .count()
}

#[derive(Clone, Copy)]
pub(super) struct RuntimeLeaseAcquisition<'request, 'payload> {
    pub(super) request: &'request ValidatedRuntimeRequest<'payload>,
    pub(super) request_id: RequestId,
    pub(super) instance_alias: &'request str,
    pub(super) holder_id: actingcommand_contract::HolderId,
    pub(super) connection_id: ConnectionId,
    pub(super) run_links: Option<RuntimeRunLinks>,
    pub(super) lease_ttl_ms: Option<u64>,
}

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
        self.require_bound_endpoint(
            &resolved,
            self.events
                .request_links(request, Some(resolved.instance_id()), None, None),
            EventAction::LeaseAcquire,
        )?;
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

impl HostShared {
    pub(super) fn acquire_lease(
        &self,
        acquisition: RuntimeLeaseAcquisition<'_, '_>,
    ) -> Result<OperationSuccess, RequestFailure> {
        let RuntimeLeaseAcquisition {
            request,
            request_id,
            instance_alias,
            holder_id,
            connection_id,
            run_links,
            lease_ttl_ms,
        } = acquisition;
        let resolved = self.resolve_instance(instance_alias)?;
        let instance_guard = self.instance_guard(resolved.instance_id())?;
        let _admission = lock(&instance_guard, "lock_instance_admission")?;
        self.expire_instance_if_due(resolved.instance_id())?;
        self.require_bound_endpoint(
            &resolved,
            self.events
                .request_links(request, Some(resolved.instance_id()), None, None),
            EventAction::LeaseAcquire,
        )?;
        let preparation = {
            let mut scheduler = lock(&self.scheduler, "prepare_lease")?;
            let now_monotonic_ms = self.monotonic_ms()?;
            match lease_ttl_ms {
                Some(lease_ttl_ms) => scheduler.prepare_acquire_with_ttl(
                    request_id,
                    resolved.instance_id(),
                    holder_id,
                    connection_id,
                    lease_ttl_ms,
                    now_monotonic_ms,
                ),
                None => scheduler.prepare_acquire(
                    request_id,
                    resolved.instance_id(),
                    holder_id,
                    connection_id,
                    now_monotonic_ms,
                ),
            }
        };
        let preparation = match preparation {
            Ok(preparation) => preparation,
            Err(error) => {
                self.append_lease_requested(request, &resolved)?;
                return Err(self.scheduler_denied(request, &resolved, None, error)?);
            }
        };
        self.grant_prepared_lease(request, request_id, &resolved, preparation, run_links)
    }

    fn grant_prepared_lease(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        request_id: RequestId,
        resolved: &RegisteredInstance,
        preparation: LeasePreparation,
        run_links: Option<RuntimeRunLinks>,
    ) -> Result<OperationSuccess, RequestFailure> {
        if preparation.is_existing() {
            let terminal = self.existing_lease_terminal(
                request_id,
                preparation.token().lease_id(),
                EventType::LeaseGranted,
            )?;
            return Ok(OperationSuccess {
                state: RuntimeReceiptState::Admitted,
                terminal: Some(terminal),
                result: RuntimeResult::LeaseGranted {
                    token: preparation.token().clone(),
                },
            });
        }
        self.append_lease_requested(request, resolved)?;
        self.append_scheduler_admitted(request, resolved, None)?;
        let token = preparation.token().clone();
        let action_id = self
            .events
            .action_id()
            .map_err(RequestFailure::poison_without_terminal)?;
        let mut links = self.events.request_links(
            request,
            Some(resolved.instance_id()),
            Some(token.lease_id()),
            Some(action_id),
        );
        if let Some(run_links) = run_links {
            links = run_links.apply(links);
        }
        self.grant_prepared_lease_with_links(resolved, preparation, links, CapacityUse::Business)
    }

    pub(super) fn grant_prepared_lease_with_links(
        &self,
        resolved: &RegisteredInstance,
        preparation: LeasePreparation,
        links: EventLinksDraft,
        capacity_use: CapacityUse,
    ) -> Result<OperationSuccess, RequestFailure> {
        if matches!(capacity_use, CapacityUse::Business) && !preparation.is_existing() {
            self.require_business_capacity(links.clone())?;
        }
        let intent = self.lease_intent(
            EventAction::LeaseAcquire,
            links.clone(),
            resolved.audit_endpoint(),
        )?;
        let plan = CriticalEventPlan::new(
            CriticalOperation::LeaseTransition(LeaseTransitionTarget::Granted),
            intent,
        )
        .map_err(|_| RequestFailure::poison_without_terminal(critical_plan_error()))?;
        let endpoint = resolved.audit_endpoint().to_string();
        let outcome_links = links.clone();
        let failure_links = links;
        let result = execute_critical(
            &self.ledger,
            self.events.fingerprinter(),
            plan,
            || match self.commit_acquire(preparation) {
                Ok(token) => CriticalActionReport::Succeeded {
                    value: token,
                    effect: DefiniteEffectDisposition::Performed,
                },
                Err(error) => CriticalActionReport::Failed {
                    effect: error.effect,
                    error,
                },
            },
            |_, effect| {
                self.events
                    .draft(
                        EventSeverity::Info,
                        EventSource::Scheduler,
                        OriginModule::Scheduler,
                        EventActor::Scheduler,
                        outcome_links,
                        LeasePayloadDraft::granted(
                            EventAction::LeaseAcquire,
                            effect.into(),
                            audit_endpoint(&endpoint),
                        ),
                    )
                    .map_err(|_| actingcommand_contract::SanitizationError::fingerprinter_failure())
            },
            |error, effect| {
                self.events
                    .draft(
                        EventSeverity::Error,
                        EventSource::Scheduler,
                        OriginModule::Scheduler,
                        EventActor::Scheduler,
                        failure_links,
                        LeasePayloadDraft::transition_failed(
                            EventAction::LeaseAcquire,
                            error.diagnostic,
                            effect,
                            audit_endpoint(&endpoint),
                        ),
                    )
                    .map_err(|_| actingcommand_contract::SanitizationError::fingerprinter_failure())
            },
        );
        self.map_critical_lease_result(result, RuntimeReceiptState::Admitted, |token| {
            RuntimeResult::LeaseGranted { token }
        })
    }

    fn existing_lease_terminal(
        &self,
        request_id: RequestId,
        lease_id: LeaseId,
        event_type: EventType,
    ) -> Result<TerminalEvent, RequestFailure> {
        let events = self
            .ledger
            .query(EventQuery {
                event_type: Some(event_type),
                request_id: Some(request_id),
                lease_id: Some(lease_id),
                ..EventQuery::default()
            })
            .map_err(|_| {
                RequestFailure::poison_without_terminal(ledger_error("query_lease_terminal"))
            })?;
        match events.as_slice() {
            [event] => Ok(terminal(event)),
            [] => Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "lease_terminal_event_missing",
                    "recover_idempotent_lease_request",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            )),
            _ => Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "lease_terminal_event_duplicated",
                    "recover_idempotent_lease_request",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            )),
        }
    }

    fn existing_request_terminal(
        &self,
        request_id: RequestId,
        event_type: EventType,
    ) -> Result<TerminalEvent, RequestFailure> {
        self.query_single_terminal(request_id, None, event_type)?
            .ok_or_else(|| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    "request_terminal_event_missing",
                    "recover_idempotent_runtime_request",
                    RuntimeErrorCode::RuntimeFatal,
                ))
            })
    }

    fn query_single_terminal(
        &self,
        request_id: RequestId,
        lease_id: Option<LeaseId>,
        event_type: EventType,
    ) -> Result<Option<TerminalEvent>, RequestFailure> {
        let events = self
            .ledger
            .query(EventQuery {
                event_type: Some(event_type),
                request_id: Some(request_id),
                lease_id,
                ..EventQuery::default()
            })
            .map_err(|_| {
                RequestFailure::poison_without_terminal(ledger_error("query_request_terminal"))
            })?;
        match events.as_slice() {
            [] => Ok(None),
            [event] => Ok(Some(terminal(event))),
            _ => Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "request_terminal_event_duplicated",
                    "recover_idempotent_runtime_request",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            )),
        }
    }

    pub(super) fn renew_lease(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        request_id: RequestId,
        token: &LeaseToken,
        connection_id: ConnectionId,
    ) -> Result<OperationSuccess, RequestFailure> {
        let replayed = lock(&self.scheduler, "replay_renew_lease").and_then(|scheduler| {
            scheduler
                .replayed_renew(request_id, token, connection_id)
                .map_err(|error| RuntimeHostError::scheduler("replay_renew_lease", &error))
        });
        let replayed = match replayed {
            Ok(replayed) => replayed,
            Err(error) => {
                return Err(self.scheduler_denied_error(
                    request,
                    Some(token.instance_id()),
                    Some(token.lease_id()),
                    "",
                    error,
                )?);
            }
        };
        if let Some(renewed) = replayed {
            let terminal = self.existing_lease_terminal(
                request_id,
                renewed.lease_id(),
                EventType::LeaseRenewed,
            )?;
            return Ok(OperationSuccess {
                state: RuntimeReceiptState::Completed,
                terminal: Some(terminal),
                result: RuntimeResult::LeaseRenewed { token: renewed },
            });
        }
        let instance_guard = self.instance_guard(token.instance_id())?;
        let _admission = lock(&instance_guard, "lock_instance_admission")?;
        let resolved = self.validated_instance(request, token, connection_id)?;
        self.append_scheduler_admitted_for_token(request, token, resolved.audit_endpoint())?;
        let action_id = self
            .events
            .action_id()
            .map_err(RequestFailure::poison_without_terminal)?;
        let links = self.events.request_links(
            request,
            Some(token.instance_id()),
            Some(token.lease_id()),
            Some(action_id),
        );
        let intent = self.lease_intent(
            EventAction::LeaseRenew,
            links.clone(),
            resolved.audit_endpoint(),
        )?;
        let plan = CriticalEventPlan::new(
            CriticalOperation::LeaseTransition(LeaseTransitionTarget::Renewed),
            intent,
        )
        .map_err(|_| RequestFailure::poison_without_terminal(critical_plan_error()))?;
        let outcome_links = links.clone();
        let failure_links = links;
        let endpoint = resolved.audit_endpoint;
        let result = execute_critical(
            &self.ledger,
            self.events.fingerprinter(),
            plan,
            || {
                let renewed = lock(&self.scheduler, "renew_lease").and_then(|mut scheduler| {
                    scheduler
                        .renew(request_id, token, connection_id, self.monotonic_ms()?)
                        .map_err(|error| RuntimeHostError::scheduler("renew_lease", &error))
                });
                match renewed {
                    Ok(token) => CriticalActionReport::Succeeded {
                        value: token,
                        effect: DefiniteEffectDisposition::Performed,
                    },
                    Err(error) => CriticalActionReport::Failed {
                        error: ActionFailure::scheduler(error),
                        effect: EffectDisposition::NotPerformed,
                    },
                }
            },
            |_, effect| {
                self.lease_outcome_draft(
                    EventSeverity::Info,
                    outcome_links,
                    LeasePayloadDraft::renewed(
                        EventAction::LeaseRenew,
                        effect.into(),
                        audit_endpoint(&endpoint),
                    ),
                )
            },
            |error, effect| {
                self.lease_failure_draft(
                    failure_links,
                    EventAction::LeaseRenew,
                    error.diagnostic,
                    effect,
                    &endpoint,
                )
            },
        );
        self.map_critical_lease_result(result, RuntimeReceiptState::Completed, |token| {
            RuntimeResult::LeaseRenewed { token }
        })
    }

    pub(super) fn release_lease(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        request_id: RequestId,
        token: &LeaseToken,
        connection_id: ConnectionId,
        run_links: Option<RuntimeRunLinks>,
    ) -> Result<OperationSuccess, RequestFailure> {
        let replayed = lock(&self.scheduler, "replay_release_lease").and_then(|scheduler| {
            scheduler
                .replayed_release(request_id, token, connection_id)
                .map_err(|error| RuntimeHostError::scheduler("replay_release_lease", &error))
        });
        let replayed = match replayed {
            Ok(replayed) => replayed,
            Err(error) => {
                return Err(self.scheduler_denied_error(
                    request,
                    Some(token.instance_id()),
                    Some(token.lease_id()),
                    "",
                    error,
                )?);
            }
        };
        if let Some(released) = replayed {
            let terminal = self.existing_lease_terminal(
                request_id,
                released.token.lease_id(),
                EventType::LeaseReleased,
            )?;
            return Ok(OperationSuccess {
                state: RuntimeReceiptState::Completed,
                terminal: Some(terminal),
                result: RuntimeResult::LeaseReleased {
                    instance_id: released.token.instance_id(),
                    lease_id: released.token.lease_id(),
                },
            });
        }
        let resolved = self.validated_instance(request, token, connection_id)?;
        let instance_guard = self.instance_guard(token.instance_id())?;
        let _admission = lock(&instance_guard, "lock_instance_admission")?;
        self.expire_queued_for_instance(token.instance_id())?;
        self.close_instance_resources(token, connection_id, EventLinksDraft::default())?;
        let transfer = lock(&self.scheduler, "prepare_release_transfer")?
            .prepare_transfer(
                token,
                connection_id,
                LeaseTransferReason::ExplicitRelease,
                Some(request_id),
                self.monotonic_ms()?,
            )
            .map_err(|error| {
                RequestFailure::poison_without_terminal(RuntimeHostError::scheduler(
                    "prepare_release_transfer",
                    &error,
                ))
            })?;
        self.append_scheduler_admitted_for_token(request, token, resolved.audit_endpoint())?;
        match transfer {
            TransferPreparation::Ready(prepared) => {
                if self.capacity_allows_transfer(token)? {
                    return self
                        .release_via_transfer(request, token, &resolved, prepared, run_links);
                }
            }
            TransferPreparation::Deferred => {
                return Err(self.scheduler_denied_error(
                    request,
                    Some(token.instance_id()),
                    Some(token.lease_id()),
                    resolved.audit_endpoint(),
                    RuntimeHostError::scheduler(
                        "prepare_release_transfer",
                        &SchedulerError::TransferNotSafe,
                    ),
                )?);
            }
            TransferPreparation::NoCandidate => {}
        }
        let action_id = self
            .events
            .action_id()
            .map_err(RequestFailure::poison_without_terminal)?;
        let mut links = self.events.request_links(
            request,
            Some(token.instance_id()),
            Some(token.lease_id()),
            Some(action_id),
        );
        if let Some(run_links) = run_links {
            links = run_links.apply(links);
        }
        let intent = self.lease_intent(
            EventAction::LeaseRelease,
            links.clone(),
            resolved.audit_endpoint(),
        )?;
        let plan = CriticalEventPlan::new(
            CriticalOperation::LeaseTransition(LeaseTransitionTarget::Released),
            intent,
        )
        .map_err(|_| RequestFailure::poison_without_terminal(critical_plan_error()))?;
        let endpoint = resolved.audit_endpoint.clone();
        let outcome_links = links.clone();
        let failure_links = links;
        let result = execute_critical(
            &self.ledger,
            self.events.fingerprinter(),
            plan,
            || match self.complete_explicit_release(request_id, token, connection_id) {
                Ok(token) => CriticalActionReport::Succeeded {
                    value: token,
                    effect: DefiniteEffectDisposition::Performed,
                },
                Err(error) => CriticalActionReport::Failed {
                    effect: error.effect,
                    error,
                },
            },
            |_, effect| {
                self.lease_outcome_draft(
                    EventSeverity::Info,
                    outcome_links,
                    LeasePayloadDraft::released(
                        EventAction::LeaseRelease,
                        effect.into(),
                        audit_endpoint(&endpoint),
                    ),
                )
            },
            |error, effect| {
                self.lease_failure_draft(
                    failure_links,
                    EventAction::LeaseRelease,
                    error.diagnostic,
                    effect,
                    &endpoint,
                )
            },
        );
        self.map_critical_lease_result(result, RuntimeReceiptState::Completed, |token| {
            RuntimeResult::LeaseReleased {
                instance_id: token.instance_id(),
                lease_id: token.lease_id(),
            }
        })
    }

    pub(super) fn validated_instance(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        token: &LeaseToken,
        connection_id: ConnectionId,
    ) -> Result<RegisteredInstance, RequestFailure> {
        let validation = lock(&self.scheduler, "validate_runtime_lease").and_then(|scheduler| {
            scheduler
                .validate_write(token, connection_id, self.monotonic_ms()?)
                .map_err(|error| RuntimeHostError::scheduler("validate_runtime_lease", &error))
        });
        if let Err(error) = validation {
            return Err(self.scheduler_denied_error(
                request,
                Some(token.instance_id()),
                Some(token.lease_id()),
                "",
                error,
            )?);
        }
        lock(&self.registered_instances, "read_instance_registry")?
            .get(&token.instance_id())
            .cloned()
            .ok_or_else(|| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    "active_lease_instance_missing",
                    "read_instance_registry",
                    RuntimeErrorCode::RuntimeFatal,
                ))
            })
    }

    pub(super) fn transfer_preempted_if_ready(
        &self,
        token: &LeaseToken,
        connection_id: ConnectionId,
    ) -> Result<bool, RequestFailure> {
        let instance_guard = self.instance_guard(token.instance_id())?;
        let admission = lock(&instance_guard, "lock_instance_admission")?;
        self.transfer_preempted_while_guarded(token, connection_id, &admission)
    }

    pub(super) fn transfer_preempted_while_guarded(
        &self,
        token: &LeaseToken,
        connection_id: ConnectionId,
        _admission: &MutexGuard<'_, ()>,
    ) -> Result<bool, RequestFailure> {
        self.expire_queued_for_instance(token.instance_id())?;
        let transfer = lock(&self.scheduler, "prepare_preempted_transfer")?
            .prepare_transfer(
                token,
                connection_id,
                LeaseTransferReason::Preempted,
                None,
                self.monotonic_ms()?,
            )
            .map_err(|error| {
                RequestFailure::poison_without_terminal(RuntimeHostError::scheduler(
                    "prepare_preempted_transfer",
                    &error,
                ))
            })?;
        match transfer {
            TransferPreparation::NoCandidate => Ok(false),
            TransferPreparation::Ready(prepared) => {
                if !self.capacity_allows_transfer(token)? {
                    self.cleanup_token_inner(
                        token,
                        connection_id,
                        LeaseReleaseReason::Preempted,
                        None,
                        Some(_admission),
                    )?;
                    return Ok(true);
                }
                self.perform_transfer(prepared).map(|_| true)
            }
            TransferPreparation::Deferred => Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "preempted_transfer_remained_destructive",
                    "prepare_preempted_transfer",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            )),
        }
    }

    fn commit_acquire(&self, preparation: LeasePreparation) -> Result<LeaseToken, ActionFailure> {
        let token = preparation.token().clone();
        let now = self.monotonic_ms().map_err(ActionFailure::poison)?;
        let mut scheduler = lock(&self.scheduler, "commit_lease").map_err(ActionFailure::poison)?;
        if let Err(error) = scheduler.commit_acquire(preparation, now) {
            return Err(ActionFailure::scheduler(RuntimeHostError::scheduler(
                "commit_lease",
                &error,
            )));
        }
        let protected = scheduler.protected_instance_ids(now);
        if let Err(error) = lock(&self.owner, "update_owner_file")
            .and_then(|mut owner| owner.set_active_instances(protected))
        {
            let rollback = scheduler.rollback_lease(&token).err();
            let rollback_error = rollback
                .map(|rollback| RuntimeHostError::scheduler("rollback_lease", &rollback))
                .unwrap_or(error);
            return Err(ActionFailure::poison(rollback_error));
        }
        Ok(token)
    }

    fn complete_explicit_release(
        &self,
        request_id: RequestId,
        token: &LeaseToken,
        connection_id: ConnectionId,
    ) -> Result<LeaseToken, ActionFailure> {
        {
            let mut scheduler =
                lock(&self.scheduler, "release_lease").map_err(ActionFailure::poison)?;
            scheduler
                .release(
                    request_id,
                    token,
                    connection_id,
                    self.monotonic_ms().map_err(ActionFailure::poison)?,
                )
                .map_err(|error| {
                    ActionFailure::scheduler(RuntimeHostError::scheduler("release_lease", &error))
                })?;
        }
        self.persist_active_instances()
            .map_err(ActionFailure::poison)?;
        Ok(token.clone())
    }

    fn release_via_transfer(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        token: &LeaseToken,
        resolved: &RegisteredInstance,
        prepared: Box<PreparedLeaseTransfer>,
        run_links: Option<RuntimeRunLinks>,
    ) -> Result<OperationSuccess, RequestFailure> {
        let action_id = self
            .events
            .action_id()
            .map_err(RequestFailure::poison_without_terminal)?;
        let mut links = self.events.request_links(
            request,
            Some(token.instance_id()),
            Some(token.lease_id()),
            Some(action_id),
        );
        if let Some(run_links) = run_links {
            links = run_links.apply(links);
        }
        self.append_event(
            EventSeverity::Info,
            EventSource::Scheduler,
            OriginModule::Scheduler,
            EventActor::Scheduler,
            links.clone(),
            LeasePayloadDraft::transition_intent(
                EventAction::LeaseRelease,
                audit_endpoint(resolved.audit_endpoint()),
            ),
        )?;
        self.perform_transfer(prepared)?;
        let released = self.append_event(
            EventSeverity::Info,
            EventSource::Scheduler,
            OriginModule::Scheduler,
            EventActor::Scheduler,
            links,
            LeasePayloadDraft::released(
                EventAction::LeaseRelease,
                EffectDisposition::Performed,
                audit_endpoint(resolved.audit_endpoint()),
            ),
        )?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(terminal(&released)),
            result: RuntimeResult::LeaseReleased {
                instance_id: token.instance_id(),
                lease_id: token.lease_id(),
            },
        })
    }

    pub(super) fn cleanup_token(
        &self,
        token: &LeaseToken,
        connection_id: ConnectionId,
        reason: LeaseReleaseReason,
    ) -> RuntimeHostResult<()> {
        self.cleanup_token_inner(token, connection_id, reason, None, None)
    }

    /// The sole producer of a run-linked scheduled failure cleanup.
    ///
    /// Its fixed `BackendFailure` reason is intentionally not caller-selectable: the resulting
    /// full run chain is the bounded proof consumed by startup settlement recovery.
    pub(super) fn cleanup_scheduled_failure_with_run_links(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        token: &LeaseToken,
        connection_id: ConnectionId,
        run_links: RuntimeRunLinks,
    ) -> RuntimeHostResult<()> {
        self.cleanup_token_inner(
            token,
            connection_id,
            LeaseReleaseReason::BackendFailure,
            Some((request, run_links)),
            None,
        )
    }

    pub(super) fn cleanup_token_inner(
        &self,
        token: &LeaseToken,
        connection_id: ConnectionId,
        reason: LeaseReleaseReason,
        request_links: Option<(&ValidatedRuntimeRequest<'_>, RuntimeRunLinks)>,
        admission: Option<&MutexGuard<'_, ()>>,
    ) -> RuntimeHostResult<()> {
        let resolved = lock(&self.registered_instances, "read_instance_registry")?
            .get(&token.instance_id())
            .cloned();
        let Some(resolved) = resolved else {
            let active = lock(&self.scheduler, "check_cleanup_lease")?
                .active_tokens()
                .into_iter()
                .any(|active| active == *token);
            return if active {
                Err(RuntimeHostError::fatal(
                    "active_lease_instance_missing",
                    "cleanup_runtime_connection",
                    RuntimeErrorCode::RuntimeFatal,
                ))
            } else {
                Ok(())
            };
        };
        let instance_guard = self
            .instance_guard(token.instance_id())
            .map_err(|failure| *failure.error)?;
        let _owned_admission = if admission.is_none() {
            Some(lock(&instance_guard, "lock_instance_admission")?)
        } else {
            None
        };
        self.expire_queued_for_instance(token.instance_id())
            .map_err(|failure| *failure.error)?;
        self.close_instance_resources(token, connection_id, EventLinksDraft::default())
            .map_err(|failure| *failure.error)?;
        let transfer_reason = match reason {
            LeaseReleaseReason::Disconnect => Some(LeaseTransferReason::Disconnect),
            LeaseReleaseReason::Expired => Some(LeaseTransferReason::Expired),
            LeaseReleaseReason::Explicit
            | LeaseReleaseReason::Preempted
            | LeaseReleaseReason::BackendFailure
            | LeaseReleaseReason::HostShutdown => None,
        };
        if let Some(transfer_reason) = transfer_reason {
            let transfer = lock(&self.scheduler, "prepare_cleanup_transfer")?
                .prepare_transfer(
                    token,
                    connection_id,
                    transfer_reason,
                    None,
                    self.monotonic_ms()?,
                )
                .map_err(|error| RuntimeHostError::scheduler("prepare_cleanup_transfer", &error))?;
            match transfer {
                TransferPreparation::Ready(prepared) => {
                    if self.capacity_allows_transfer(token)? {
                        self.cleanup_via_transfer(token, &resolved, reason, prepared)?;
                        return Ok(());
                    }
                }
                TransferPreparation::Deferred if reason == LeaseReleaseReason::Expired => {
                    return Ok(());
                }
                TransferPreparation::Deferred => {
                    return Err(RuntimeHostError::fatal(
                        "cleanup_transfer_remained_destructive",
                        "prepare_cleanup_transfer",
                        RuntimeErrorCode::RuntimeFatal,
                    ));
                }
                TransferPreparation::NoCandidate => {}
            }
        }
        if matches!(
            reason,
            LeaseReleaseReason::BackendFailure | LeaseReleaseReason::HostShutdown
        ) {
            self.cancel_instance_queue(
                token.instance_id(),
                if reason == LeaseReleaseReason::BackendFailure {
                    DiagnosticCode::BackendOperationFailed
                } else {
                    DiagnosticCode::LeaseQueueDisconnected
                },
            )
            .map_err(|failure| *failure.error)?;
        }
        let action_id = self.events.action_id()?;
        let links = match request_links {
            Some((request, run_links)) => run_links.apply(self.events.request_links(
                request,
                Some(token.instance_id()),
                Some(token.lease_id()),
                Some(action_id),
            )),
            None => self.events.synthetic_links(token, action_id)?,
        };
        let target = if reason == LeaseReleaseReason::Expired {
            LeaseTransitionTarget::Expired
        } else {
            LeaseTransitionTarget::Released
        };
        let action = if reason == LeaseReleaseReason::Expired {
            EventAction::LeaseExpire
        } else {
            EventAction::LeaseRelease
        };
        let intent = self
            .lease_intent(action, links.clone(), resolved.audit_endpoint())
            .map_err(|failure| *failure.error)?;
        let plan = CriticalEventPlan::new(CriticalOperation::LeaseTransition(target), intent)
            .map_err(|_| critical_plan_error())?;
        let endpoint = resolved.audit_endpoint;
        let outcome_links = links.clone();
        let failure_links = links;
        let result = execute_critical(
            &self.ledger,
            self.events.fingerprinter(),
            plan,
            || {
                let released = {
                    let mut scheduler = match lock(&self.scheduler, "cleanup_runtime_lease") {
                        Ok(scheduler) => scheduler,
                        Err(error) => {
                            return CriticalActionReport::Failed {
                                error: ActionFailure::poison(error),
                                effect: EffectDisposition::Indeterminate,
                            };
                        }
                    };
                    if reason == LeaseReleaseReason::Expired {
                        let now = match self.monotonic_ms() {
                            Ok(now) => now,
                            Err(error) => {
                                return CriticalActionReport::Failed {
                                    error: ActionFailure::poison(error),
                                    effect: EffectDisposition::Indeterminate,
                                };
                            }
                        };
                        scheduler.expire_token(token, now)
                    } else {
                        scheduler.release_owned(token, connection_id, reason)
                    }
                };
                match released {
                    Ok(_) => match self.persist_active_instances() {
                        Ok(()) => CriticalActionReport::Succeeded {
                            value: token.clone(),
                            effect: DefiniteEffectDisposition::Performed,
                        },
                        Err(error) => CriticalActionReport::Failed {
                            effect: EffectDisposition::Indeterminate,
                            error: ActionFailure::poison(error),
                        },
                    },
                    Err(SchedulerError::LeaseMissing | SchedulerError::LeaseMismatch) => {
                        let already_removed =
                            lock(&self.scheduler, "check_scheduler_cleanup").map(|scheduler| {
                                !scheduler
                                    .active_tokens()
                                    .into_iter()
                                    .any(|active| active == *token)
                            });
                        match already_removed {
                            Ok(true) => CriticalActionReport::Succeeded {
                                value: token.clone(),
                                effect: DefiniteEffectDisposition::NotPerformed,
                            },
                            Ok(false) => CriticalActionReport::Failed {
                                error: ActionFailure::poison(RuntimeHostError::fatal(
                                    "scheduler_cleanup_state_mismatch",
                                    "cleanup_runtime_lease",
                                    RuntimeErrorCode::RuntimeFatal,
                                )),
                                effect: EffectDisposition::Indeterminate,
                            },
                            Err(error) => CriticalActionReport::Failed {
                                error: ActionFailure::poison(error),
                                effect: EffectDisposition::Indeterminate,
                            },
                        }
                    }
                    Err(error) => CriticalActionReport::Failed {
                        error: ActionFailure::scheduler(RuntimeHostError::scheduler(
                            "cleanup_runtime_lease",
                            &error,
                        )),
                        effect: EffectDisposition::NotPerformed,
                    },
                }
            },
            |_, effect| {
                self.lease_outcome_draft(
                    EventSeverity::Info,
                    outcome_links,
                    if reason == LeaseReleaseReason::Expired {
                        LeasePayloadDraft::expired(action, effect.into(), audit_endpoint(&endpoint))
                    } else {
                        LeasePayloadDraft::released(
                            action,
                            effect.into(),
                            audit_endpoint(&endpoint),
                        )
                    },
                )
            },
            |error, effect| {
                self.lease_failure_draft(failure_links, action, error.diagnostic, effect, &endpoint)
            },
        );
        match result {
            Ok(_) => Ok(()),
            Err(CriticalExecutionError::Action { error, outcome, .. }) => {
                let _ = error
                    .error
                    .lifecycle
                    .recorded_event
                    .set(*outcome.event_id());
                if error.poison_runtime {
                    self.fatal.mark(error.error.clone())?;
                }
                Err(error.error)
            }
            Err(error) => {
                let error = critical_execution_error(&error);
                self.fatal.mark(error.clone())?;
                Err(error)
            }
        }
    }

    fn cleanup_via_transfer(
        &self,
        token: &LeaseToken,
        resolved: &RegisteredInstance,
        reason: LeaseReleaseReason,
        prepared: Box<PreparedLeaseTransfer>,
    ) -> RuntimeHostResult<()> {
        let action_id = self.events.action_id()?;
        let links = self.events.synthetic_links(token, action_id)?;
        let action = if reason == LeaseReleaseReason::Expired {
            EventAction::LeaseExpire
        } else {
            EventAction::LeaseRelease
        };
        self.append_event(
            EventSeverity::Info,
            EventSource::Scheduler,
            OriginModule::Scheduler,
            EventActor::Scheduler,
            links.clone(),
            LeasePayloadDraft::transition_intent(action, audit_endpoint(resolved.audit_endpoint())),
        )
        .map_err(|failure| *failure.error)?;
        self.perform_transfer(prepared)
            .map_err(|failure| *failure.error)?;
        self.append_event(
            EventSeverity::Info,
            EventSource::Scheduler,
            OriginModule::Scheduler,
            EventActor::Scheduler,
            links,
            if reason == LeaseReleaseReason::Expired {
                LeasePayloadDraft::expired(
                    action,
                    EffectDisposition::Performed,
                    audit_endpoint(resolved.audit_endpoint()),
                )
            } else {
                LeasePayloadDraft::released(
                    action,
                    EffectDisposition::Performed,
                    audit_endpoint(resolved.audit_endpoint()),
                )
            },
        )
        .map_err(|failure| *failure.error)?;
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn durable_lease_expiry_terminal_for_test(
        &self,
        token: &LeaseToken,
    ) -> RuntimeHostResult<Option<TerminalEvent>> {
        let through_sequence = self
            .ledger
            .latest_sequence()
            .map_err(|_| ledger_error("read_test_lease_expiry_position"))?;
        let mut selected_terminal = None;
        for event_type in [EventType::LeaseExpired, EventType::LeaseReleased] {
            let events = self
                .ledger
                .query_page(
                    EventQuery {
                        to_sequence: Some(through_sequence),
                        event_type: Some(event_type),
                        instance_id: Some(token.instance_id()),
                        lease_id: Some(token.lease_id()),
                        ..EventQuery::default()
                    },
                    0,
                    through_sequence,
                    2,
                )
                .map_err(|_| ledger_error("read_test_lease_expiry_terminal"))?;
            let event = match events.as_slice() {
                [] => None,
                [event] => Some(terminal(event)),
                _ => {
                    return Err(RuntimeHostError::fatal(
                        "test_lease_expiry_terminal_not_unique",
                        "expire_lease_once_for_test",
                        RuntimeErrorCode::RuntimeFatal,
                    ));
                }
            };
            // LeaseExpired is the exact scan result. LeaseReleased is the permitted fallback
            // for an already-cleaned token and must not replace a durable expiry on replay.
            selected_terminal = selected_terminal.or(event);
        }
        Ok(selected_terminal)
    }

    #[cfg(test)]
    fn active_lease_token_for_test(
        &self,
        token: &LeaseToken,
    ) -> RuntimeHostResult<Option<LeaseToken>> {
        lock(&self.scheduler, "read_test_lease_expiry_token").map(|scheduler| {
            scheduler.active_tokens().into_iter().find(|active| {
                active.instance_id() == token.instance_id() && active.lease_id() == token.lease_id()
            })
        })
    }

    #[cfg(test)]
    pub(super) fn record_completed_lease_expiry_for_test(
        &self,
        token: &LeaseToken,
    ) -> RuntimeHostResult<TerminalEvent> {
        let terminal = self
            .durable_lease_expiry_terminal_for_test(token)?
            .ok_or_else(|| {
                RuntimeHostError::fatal(
                    "test_lease_expiry_terminal_missing",
                    "expire_lease_once_for_test",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
        if self.active_lease_token_for_test(token)?.is_some() {
            return Err(RuntimeHostError::fatal(
                "test_lease_expiry_token_cleanup_incomplete",
                "expire_lease_once_for_test",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        let mut checkpoints = lock(
            &self.lease_expiry_test_checkpoints,
            "record_test_lease_expiry_checkpoint",
        )?;
        if checkpoints
            .iter()
            .any(|checkpoint| checkpoint.token == *token)
        {
            return Err(RuntimeHostError::fatal(
                "test_lease_expiry_checkpoint_duplicate",
                "expire_lease_once_for_test",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        if checkpoints
            .iter()
            .any(|checkpoint| checkpoint.terminal == terminal)
        {
            return Err(RuntimeHostError::fatal(
                "test_lease_expiry_checkpoint_inconsistent",
                "expire_lease_once_for_test",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        checkpoints.push(LeaseExpiryTestCheckpoint {
            token: token.clone(),
            terminal,
        });
        Ok(terminal)
    }

    #[cfg(test)]
    pub(super) fn replay_lease_expiry_checkpoint_for_test(
        &self,
        token: &LeaseToken,
    ) -> RuntimeHostResult<Option<TerminalEvent>> {
        let checkpoint = {
            let checkpoints = lock(
                &self.lease_expiry_test_checkpoints,
                "read_test_lease_expiry_checkpoint",
            )?;
            let mut candidates = checkpoints.iter().filter(|checkpoint| {
                lease_token_identity_match_count(&checkpoint.token, token) >= 4
            });
            let Some(checkpoint) = candidates.next() else {
                return Ok(None);
            };
            if candidates.next().is_some() {
                return Err(RuntimeHostError::fatal(
                    "test_lease_expiry_checkpoint_duplicate",
                    "expire_lease_once_for_test",
                    RuntimeErrorCode::RuntimeFatal,
                ));
            }
            if checkpoint.token != *token {
                return Err(RuntimeHostError::fatal(
                    "test_lease_expiry_token_identity_mismatch",
                    "expire_lease_once_for_test",
                    RuntimeErrorCode::RuntimeFatal,
                ));
            }
            checkpoint.clone()
        };
        let durable_terminal = self
            .durable_lease_expiry_terminal_for_test(&checkpoint.token)?
            .ok_or_else(|| {
                RuntimeHostError::fatal(
                    "test_lease_expiry_checkpoint_missing",
                    "expire_lease_once_for_test",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
        if durable_terminal != checkpoint.terminal {
            return Err(RuntimeHostError::fatal(
                "test_lease_expiry_checkpoint_inconsistent",
                "expire_lease_once_for_test",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        if self
            .active_lease_token_for_test(&checkpoint.token)?
            .is_some()
        {
            return Err(RuntimeHostError::fatal(
                "test_lease_expiry_terminal_token_still_active",
                "expire_lease_once_for_test",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        Ok(Some(checkpoint.terminal))
    }

    pub(super) fn expire_due_leases(&self) -> RuntimeHostResult<()> {
        #[cfg(test)]
        let _scan = lock(
            &self.lease_expiry_scan_test_gate,
            "serialize_test_lease_expiry_scan",
        )?;
        self.expire_all_queued_runtime()?;
        let now = self.monotonic_ms()?;
        let (due, cooldowns_cleared) = {
            let mut scheduler = lock(&self.scheduler, "scan_expired_leases")?;
            let due = scheduler.due_tokens(now);
            let cooldowns_cleared = scheduler.clear_elapsed_cooldowns(now);
            (due, cooldowns_cleared)
        };
        if cooldowns_cleared {
            self.persist_active_instances()?;
        }
        for token in due {
            let connection_id = lock(&self.scheduler, "read_lease_connection")?
                .connection_for_token(&token)
                .map_err(|error| RuntimeHostError::scheduler("read_lease_connection", &error))?;
            self.cleanup_token(&token, connection_id, LeaseReleaseReason::Expired)?;
            #[cfg(test)]
            self.record_completed_lease_expiry_for_test(&token)?;
        }
        Ok(())
    }

    pub(super) fn expire_instance_if_due(
        &self,
        instance_id: InstanceId,
    ) -> Result<(), RequestFailure> {
        let now = self
            .monotonic_ms()
            .map_err(RequestFailure::poison_without_terminal)?;
        let due = lock(&self.scheduler, "scan_instance_expiry")?
            .due_tokens(now)
            .into_iter()
            .find(|token| token.instance_id() == instance_id);
        if let Some(token) = due {
            let connection_id = lock(&self.scheduler, "read_lease_connection")?
                .connection_for_token(&token)
                .map_err(|error| {
                    RequestFailure::poison_without_terminal(RuntimeHostError::scheduler(
                        "read_lease_connection",
                        &error,
                    ))
                })?;
            self.cleanup_token(&token, connection_id, LeaseReleaseReason::Expired)
                .map_err(RequestFailure::poison_without_terminal)?;
        }
        Ok(())
    }

    pub(super) fn cleanup_connection(
        &self,
        connection_id: ConnectionId,
        reason: LeaseReleaseReason,
    ) -> RuntimeHostResult<()> {
        lock(
            &self.governance_connections,
            "cleanup_governance_connection",
        )?
        .remove(&connection_id);
        let queued_instances = lock(&self.scheduler, "list_connection_queues")?
            .queued_instance_ids_for_connection(connection_id);
        for instance_id in queued_instances {
            let instance_guard = self
                .instance_guard(instance_id)
                .map_err(|failure| *failure.error)?;
            let _admission = lock(&instance_guard, "lock_instance_admission")?;
            let removed = lock(&self.scheduler, "cleanup_connection_queues")?
                .remove_queued_for_connection_on_instance(instance_id, connection_id)
                .map_err(|error| {
                    RuntimeHostError::scheduler("cleanup_connection_queues", &error)
                })?;
            for cancelled in removed {
                let context = self
                    .take_queued_context(&cancelled)
                    .map_err(|failure| *failure.error)?;
                self.append_queue_terminal(&context, DiagnosticCode::LeaseQueueDisconnected)
                    .map_err(|failure| *failure.error)?;
            }
        }
        let tokens =
            lock(&self.scheduler, "list_connection_leases")?.tokens_for_connection(connection_id);
        let mut failure = None;
        for token in tokens {
            if self.fatal.current()?.is_some() {
                break;
            }
            self.record_lifecycle_result(
                RuntimeLifecycleFailureStage::ConnectionCleanup,
                &mut failure,
                self.cleanup_token(&token, connection_id, reason),
            );
        }
        failure.map_or(Ok(()), Err)
    }

    pub(super) fn resolve_instance(
        &self,
        instance_alias: &str,
    ) -> Result<RegisteredInstance, RequestFailure> {
        // The identity check runs under the registry lock so an endpoint rebinding is never
        // observed half-applied.
        let registry = lock(&self.registered_instances, "read_instance_registry")?;
        let registered = registry
            .values()
            .find(|instance| instance.instance_alias == instance_alias)
            .ok_or_else(|| {
                RequestFailure::request(
                    RuntimeHostError::request(
                        "instance_unknown",
                        "resolve_runtime_instance",
                        RuntimeErrorCode::InstanceUnknown,
                    ),
                    RuntimeReceiptState::Denied,
                    None,
                )
            })?;
        self.resolve_registered_backend(registered)?;
        Ok(registered.clone())
    }

    pub(super) fn resolve_registered_backend(
        &self,
        registered: &RegisteredInstance,
    ) -> Result<crate::ResolvedExecutionInstance, RequestFailure> {
        let resolved = self
            .execution
            .resolve(&registered.instance_alias)
            .map_err(|error| {
                if error.code() == "execution_instance_unknown" {
                    RequestFailure::request(
                        RuntimeHostError::request(
                            "instance_unknown",
                            "resolve_runtime_instance",
                            RuntimeErrorCode::InstanceUnknown,
                        ),
                        RuntimeReceiptState::Denied,
                        None,
                    )
                } else {
                    RequestFailure::poison_without_terminal(RuntimeHostError::execution(
                        "resolve_runtime_instance",
                        &error,
                    ))
                }
            })?;
        if resolved.instance_id() != registered.instance_id
            || resolved.audit_endpoint() != registered.audit_endpoint
            || resolved.provenance() != registered.provenance
        {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "runtime_instance_identity_mismatch",
                    "resolve_runtime_instance",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            ));
        }
        Ok(resolved)
    }

    pub(super) fn require_physical_instance_alias(
        &self,
        instance_alias: &str,
    ) -> Result<(), RequestFailure> {
        let instance = self.resolve_instance(instance_alias)?;
        self.require_physical_provenance(&instance)
    }

    pub(super) fn require_physical_instance_id(
        &self,
        instance_id: InstanceId,
    ) -> Result<(), RequestFailure> {
        let instance = lock(&self.registered_instances, "read_instance_registry")?
            .get(&instance_id)
            .cloned()
            .ok_or_else(|| {
                RequestFailure::request(
                    RuntimeHostError::request(
                        "instance_unknown",
                        "require_physical_execution_backend",
                        RuntimeErrorCode::InstanceUnknown,
                    ),
                    RuntimeReceiptState::Denied,
                    None,
                )
            })?;
        self.require_physical_provenance(&instance)
    }

    fn require_physical_provenance(
        &self,
        instance: &RegisteredInstance,
    ) -> Result<(), RequestFailure> {
        if instance.provenance() == ExecutionBackendProvenance::PhysicalDevice {
            return Ok(());
        }
        Err(RequestFailure::request(
            RuntimeHostError::request(
                "fixture_execution_scope_forbidden",
                "require_physical_execution_backend",
                RuntimeErrorCode::InvalidRequest,
            ),
            RuntimeReceiptState::Denied,
            None,
        ))
    }

    /// Refuses a request that would open a device session on a discovery-bound instance whose
    /// ADB endpoint is still pending (the emulator is stopped): `command.rejected` plus the
    /// `runtime.failed` record naming the alias, host code `instance_not_running`, denied.
    pub(super) fn require_bound_endpoint(
        &self,
        instance: &RegisteredInstance,
        links: EventLinksDraft,
        action: EventAction,
    ) -> Result<(), RequestFailure> {
        let Err(error) = instance_not_running(instance) else {
            return Ok(());
        };
        let rejected = self.append_event(
            EventSeverity::Error,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links.clone(),
            CommandPayloadDraft::rejected(
                action,
                DiagnosticCode::RuntimeDiagnostic,
                EffectDisposition::NotPerformed,
                AuditInput::new(),
            ),
        )?;
        self.record_required_failure(&error, &rejected, links)
            .map_err(RequestFailure::poison_without_terminal)?;
        Err(RequestFailure::request(
            error,
            RuntimeReceiptState::Denied,
            Some(terminal(&rejected)),
        ))
    }

    pub(super) fn instance_guard(
        &self,
        instance_id: InstanceId,
    ) -> Result<Arc<Mutex<()>>, RequestFailure> {
        let mut guards = lock(&self.admission_guards, "read_instance_admission")?;
        Ok(Arc::clone(
            guards
                .entry(instance_id)
                .or_insert_with(|| Arc::new(Mutex::new(()))),
        ))
    }

    fn persist_active_instances(&self) -> RuntimeHostResult<()> {
        let now = self.monotonic_ms()?;
        let instances = lock(&self.scheduler, "read_active_instances")?.protected_instance_ids(now);
        lock(&self.owner, "update_owner_file")?.set_active_instances(instances)
    }

    fn append_lease_requested(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        resolved: &RegisteredInstance,
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
            LeasePayloadDraft::requested(
                EventAction::LeaseAcquire,
                audit_endpoint(resolved.audit_endpoint()),
            ),
        )
    }

    pub(super) fn append_scheduler_admitted(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        resolved: &RegisteredInstance,
        lease_id: Option<LeaseId>,
    ) -> Result<PersistedEvent, RequestFailure> {
        self.append_scheduler_admitted_for(
            request,
            resolved.instance_id(),
            lease_id,
            resolved.audit_endpoint(),
        )
    }

    pub(super) fn append_scheduler_admitted_for_token(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        token: &LeaseToken,
        endpoint: &str,
    ) -> Result<PersistedEvent, RequestFailure> {
        self.append_scheduler_admitted_for(
            request,
            token.instance_id(),
            Some(token.lease_id()),
            endpoint,
        )
    }

    fn append_scheduler_admitted_for(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        instance_id: InstanceId,
        lease_id: Option<LeaseId>,
        endpoint: &str,
    ) -> Result<PersistedEvent, RequestFailure> {
        let links = self
            .events
            .request_links(request, Some(instance_id), lease_id, None);
        self.append_event(
            EventSeverity::Info,
            EventSource::Scheduler,
            OriginModule::Scheduler,
            EventActor::Scheduler,
            links,
            SchedulerPayloadDraft::admitted(EventAction::ScheduleAdmit, audit_endpoint(endpoint)),
        )
    }

    pub(super) fn scheduler_denied(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        resolved: &RegisteredInstance,
        lease_id: Option<LeaseId>,
        error: SchedulerError,
    ) -> RuntimeHostResult<RequestFailure> {
        self.scheduler_denied_error(
            request,
            Some(resolved.instance_id()),
            lease_id,
            resolved.audit_endpoint(),
            RuntimeHostError::scheduler("scheduler_admission", &error),
        )
    }

    pub(super) fn scheduler_denied_error(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        instance_id: Option<InstanceId>,
        lease_id: Option<LeaseId>,
        endpoint: &str,
        error: RuntimeHostError,
    ) -> RuntimeHostResult<RequestFailure> {
        let links = self
            .events
            .request_links(request, instance_id, lease_id, None);
        let diagnostic = diagnostic_for_projection(error.projection());
        let event = self.append_event_raw(
            EventSeverity::Warning,
            EventSource::Scheduler,
            OriginModule::Scheduler,
            EventActor::Scheduler,
            links,
            SchedulerPayloadDraft::denied(
                EventAction::ScheduleAdmit,
                diagnostic,
                audit_endpoint(endpoint),
            ),
        )?;
        Ok(RequestFailure {
            state: RuntimeReceiptState::Denied,
            terminal: Some(terminal(&event)),
            poison_runtime: error.is_fatal(),
            error: Box::new(error),
            task_failure: None,
        })
    }

    fn lease_intent(
        &self,
        action: EventAction,
        links: EventLinksDraft,
        endpoint: &str,
    ) -> Result<actingcommand_contract::SanitizedEventDraft, RequestFailure> {
        self.events
            .draft(
                EventSeverity::Info,
                EventSource::Scheduler,
                OriginModule::Scheduler,
                EventActor::Scheduler,
                links,
                LeasePayloadDraft::transition_intent(action, audit_endpoint(endpoint)),
            )
            .and_then(|draft| self.events.sanitize(draft))
            .map_err(RequestFailure::poison_without_terminal)
    }

    fn lease_outcome_draft(
        &self,
        severity: EventSeverity,
        links: EventLinksDraft,
        payload: LeasePayloadDraft,
    ) -> Result<actingcommand_contract::EventDraft, actingcommand_contract::SanitizationError> {
        self.events
            .draft(
                severity,
                EventSource::Scheduler,
                OriginModule::Scheduler,
                EventActor::Scheduler,
                links,
                payload,
            )
            .map_err(|_| actingcommand_contract::SanitizationError::fingerprinter_failure())
    }

    fn lease_failure_draft(
        &self,
        links: EventLinksDraft,
        action: EventAction,
        diagnostic: DiagnosticCode,
        effect: EffectDisposition,
        endpoint: &str,
    ) -> Result<actingcommand_contract::EventDraft, actingcommand_contract::SanitizationError> {
        self.lease_outcome_draft(
            EventSeverity::Error,
            links,
            LeasePayloadDraft::transition_failed(
                action,
                diagnostic,
                effect,
                audit_endpoint(endpoint),
            ),
        )
    }

    fn map_critical_lease_result<T>(
        &self,
        result: Result<
            actingcommand_ledger::critical::CriticalReceipt<LeaseToken>,
            CriticalExecutionError<ActionFailure>,
        >,
        state: RuntimeReceiptState,
        result_builder: T,
    ) -> Result<OperationSuccess, RequestFailure>
    where
        T: FnOnce(LeaseToken) -> RuntimeResult,
    {
        match result {
            Ok(receipt) => {
                let terminal = terminal(receipt.outcome());
                Ok(OperationSuccess {
                    state,
                    terminal: Some(terminal),
                    result: result_builder(receipt.into_value()),
                })
            }
            Err(CriticalExecutionError::Action { error, outcome, .. }) => Err(RequestFailure {
                state: RuntimeReceiptState::Failed,
                terminal: Some(terminal(&outcome)),
                poison_runtime: error.poison_runtime,
                error: Box::new(error.error),
                task_failure: None,
            }),
            Err(error) => Err(RequestFailure::poison_without_terminal(
                critical_execution_error(&error),
            )),
        }
    }
}

/// The typed refusal of every device-facing path while a discovery binding is pending: the
/// discovered instance was stopped and has reported no ADB port yet.
pub(super) fn instance_not_running(instance: &RegisteredInstance) -> RuntimeHostResult<()> {
    if !instance.endpoint_pending() {
        return Ok(());
    }
    let mut error = RuntimeHostError::request(
        "instance_not_running",
        "require_bound_adb_endpoint",
        RuntimeErrorCode::InvalidRequest,
    )
    .with_native_detail(format!(
        "instance_alias={}; adb_endpoint=pending; start the instance with emulator control first",
        instance.instance_alias
    ));
    error.lifecycle.instance_id = Some(instance.instance_id);
    Err(error)
}
