// SPDX-License-Identifier: AGPL-3.0-only

use super::host_claims::{HostClaimWork, HostKey};
use super::recovery_ladder::{LADDER_CLAIM_TTL_MS, LadderTrigger, PendingRecoveryLadder};
use super::resource_close::LeaseDeviceEnd;
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
    /// Workflow #369 Q-2: who holds the lease once granted.
    pub(super) kind: ClaimKind,
}

#[derive(Clone)]
pub(super) struct QueuedRequestContext {
    request: RuntimeRequest,
    instance: RegisteredInstance,
    connection_id: ConnectionId,
    // Workflow #369 Q-6: where a Runtime-internal claim learns its grant; a client polls.
    grant: Option<Arc<ClaimGrantSlot>>,
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

/// Workflow #369 Q-6: where a Runtime-internal claim learns its grant. The pump or the transfer
/// that grants the claim sets the token; that never fails, so a claimant cancels its own entry
/// before it stops waiting (W-3).
#[derive(Default)]
pub(super) struct ClaimGrantSlot {
    granted: Mutex<Option<LeaseToken>>,
    ready: Condvar,
}

impl ClaimGrantSlot {
    fn grant(&self, token: LeaseToken) -> RuntimeHostResult<()> {
        *lock(&self.granted, "record_claim_grant")? = Some(token);
        self.ready.notify_all();
        Ok(())
    }

    /// The granted token, waiting at most `timeout` for it.
    pub(super) fn wait(&self, timeout: Duration) -> RuntimeHostResult<Option<LeaseToken>> {
        let granted = lock(&self.granted, "wait_claim_grant")?;
        let (granted, _) = self
            .ready
            .wait_timeout_while(granted, timeout, |granted| granted.is_none())
            .map_err(|_| lock_poison_error("wait_claim_grant"))?;
        Ok(granted.clone())
    }
}

/// Workflow #369 Q-6: an instance's admission guard that pumps the instance's queue when it is
/// dropped, so a claim queued while the guard was held is granted as soon as it can be. It
/// dereferences to the plain guard that the `_while_guarded` paths take.
pub(super) struct AdmissionGuard<'a> {
    guard: Option<MutexGuard<'a, ()>>,
    host: &'a HostShared,
    instance_id: InstanceId,
}

impl<'a> std::ops::Deref for AdmissionGuard<'a> {
    type Target = MutexGuard<'a, ()>;

    fn deref(&self) -> &Self::Target {
        match &self.guard {
            Some(guard) => guard,
            None => unreachable!("only the drop takes the admission guard"),
        }
    }
}

impl Drop for AdmissionGuard<'_> {
    fn drop(&mut self) {
        drop(self.guard.take());
        if thread::panicking() {
            return;
        }
        if let Err(error) = self.host.pump(self.instance_id) {
            self.host.mark_pump_failure(error);
        }
    }
}

/// Workflow #369 Q-2, Q-6: a Runtime-internal claim on one instance. `request` is the claim's
/// own synthetic request; its links carry the claim's waiting records (W-5).
pub(super) struct HostClaim<'a> {
    pub(super) request: &'a RuntimeRequest,
    pub(super) instance_alias: &'a str,
    pub(super) holder_id: actingcommand_contract::HolderId,
    pub(super) connection_id: ConnectionId,
    pub(super) kind: ClaimKind,
    pub(super) priority: actingcommand_contract::LeasePriority,
    pub(super) lease_ttl_ms: u64,
}

pub(super) enum HostClaimAdmission {
    Granted(LeaseToken),
    Queued {
        #[cfg_attr(
            not(test),
            allow(
                dead_code,
                reason = "the #369 S5 operator waits name the queue position in their refusal"
            )
        )]
        status: QueuedLease,
        grant: Arc<ClaimGrantSlot>,
    },
}

impl HostShared {
    /// A client `QueueLease` (Workflow #369 Q-6): the request is visible (scheduler entry,
    /// `lease.requested` + `scheduler.queued`, context) before it can be granted, and it never
    /// blocks on the admission guard before that. A free instance grants at once only when the
    /// guard is free (`try_lock`); on contention the request queues and the guard's holder pumps
    /// it when it lets the guard go.
    pub(super) fn queue_lease(
        &self,
        original: &RuntimeRequest,
        request: &ValidatedRuntimeRequest<'_>,
        instance_alias: &str,
        holder_id: actingcommand_contract::HolderId,
        policy: LeaseQueuePolicy,
        connection_id: ConnectionId,
    ) -> Result<OperationSuccess, RequestFailure> {
        let instance_id = self.resolve_instance(instance_alias)?.instance_id();
        let instance_guard = self.instance_guard(instance_id)?;
        let admission = self.try_lock_admission(&instance_guard, instance_id)?;
        let resolved = self.resolve_instance(instance_alias)?;
        if resolved.instance_id() != instance_id {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "runtime_instance_identity_mismatch",
                    "queue_lease",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            ));
        }
        if let Some(admission) = &admission {
            self.expire_instance_if_due(resolved.instance_id(), Some(&**admission))?;
        }
        self.require_bound_endpoint(
            &resolved,
            self.events
                .request_links(request, Some(resolved.instance_id()), None, None),
            EventAction::LeaseAcquire,
        )?;
        let gate = self.routine_gate(instance_id)?;
        let order_lock = self.queue_order_lock(instance_id)?;
        let order = lock(&order_lock, "lock_instance_queue_order")?;
        let outcome = lock(&self.scheduler, "queue_lease")?.request_queued_gated(
            QueueLeaseRequest::new(
                original.request_id(),
                resolved.instance_id(),
                holder_id,
                connection_id,
                policy.priority(),
                policy.timeout_ms(),
            ),
            gate,
            admission.is_some(),
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
        let queued = match decision {
            QueueAdmissionDecision::Lease(preparation) if preparation.is_existing() => {
                let token = preparation.token().clone();
                let terminal =
                    self.existing_queue_grant_terminal(original.request_id(), token.lease_id())?;
                return Ok(OperationSuccess {
                    state: RuntimeReceiptState::Admitted,
                    terminal: Some(terminal),
                    result: RuntimeResult::LeaseGranted { token },
                });
            }
            QueueAdmissionDecision::Lease(preparation) => {
                return self.grant_prepared_lease(
                    request,
                    original.request_id(),
                    &resolved,
                    preparation,
                    None,
                    CapacityUse::Business,
                );
            }
            QueueAdmissionDecision::Queued(queued) => queued,
        };
        let queued_status = |queued: &QueuedLease| {
            queued.status().map_err(|error| {
                RequestFailure::poison_without_terminal(RuntimeHostError::scheduler(
                    "queue_lease_status",
                    &error,
                ))
            })
        };
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
            let terminal =
                self.existing_request_terminal(original.request_id(), EventType::SchedulerQueued)?;
            return Ok(OperationSuccess {
                state: RuntimeReceiptState::Queued,
                terminal: Some(terminal),
                result: RuntimeResult::LeaseQueued {
                    status: queued_status(&queued)?,
                },
            });
        }
        self.append_lease_requested(request, &resolved)?;
        let mut terminal_event = self.append_scheduler_queued(request, &resolved, &queued)?;
        if queued.preempt_requested() {
            terminal_event = self.append_scheduler_preempted(request, &resolved, &queued)?;
        }
        self.register_queued_context(QueuedRequestContext {
            request: original.clone(),
            instance: resolved,
            connection_id,
            grant: None,
        })?;
        drop(order);
        if !queued.preempt_requested() && admission.is_none() {
            // Q-4: the guard's holder may have let it go before the entry was visible.
            self.pump(instance_id)?;
        }
        if queued.preempt_requested() {
            // The request is visible; an idle holder is preempted now, under the guard.
            let admission = match admission {
                Some(admission) => admission,
                None => self.lock_admission(&instance_guard, instance_id)?,
            };
            if let Some((token, transferred)) = self.promote_idle_preemption(&queued, &admission)? {
                return Ok(OperationSuccess {
                    state: RuntimeReceiptState::Admitted,
                    terminal: Some(terminal(&transferred)),
                    result: RuntimeResult::LeaseGranted { token },
                });
            }
        }
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Queued,
            terminal: Some(terminal(&terminal_event)),
            result: RuntimeResult::LeaseQueued {
                status: queued_status(&queued)?,
            },
        })
    }

    /// Workflow #369 Q-6: registers a queued request's context; the caller holds the instance's
    /// queue-order lock, so the request is not grantable before this returns.
    fn register_queued_context(&self, context: QueuedRequestContext) -> Result<(), RequestFailure> {
        if lock(&self.queued_requests, "register_queued_request")?
            .insert(context.request.request_id(), context)
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
        Ok(())
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
        let order_lock = context
            .as_ref()
            .map(|context| self.queue_order_lock(context.instance.instance_id()))
            .transpose()?;
        let _order = order_lock
            .as_ref()
            .map(|order_lock| lock(order_lock, "lock_instance_queue_order"))
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
        let order_lock = context
            .as_ref()
            .map(|context| self.queue_order_lock(context.instance.instance_id()))
            .transpose()?;
        let order = order_lock
            .as_ref()
            .map(|order_lock| lock(order_lock, "lock_instance_queue_order"))
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
        drop(order);
        self.pump(cancelled.queued().instance_id())?;
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

    /// The caller holds the instance's queue-order lock (Workflow #369 Q-6).
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
            lock(&self.host_claim_work, "forget_host_claim_work")?
                .remove(&cancelled.queued().request_id());
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

    /// Expires the instance's lapsed entries under its queue-order lock (Workflow #369 Q-6).
    pub(super) fn expire_queued_for_instance(
        &self,
        instance_id: InstanceId,
    ) -> Result<(), RequestFailure> {
        let order_lock = self.queue_order_lock(instance_id)?;
        let _order = lock(&order_lock, "lock_instance_queue_order")?;
        self.expire_queued_while_ordered(instance_id)
    }

    /// [`Self::expire_queued_for_instance`] for a caller that holds the queue-order lock.
    fn expire_queued_while_ordered(&self, instance_id: InstanceId) -> Result<(), RequestFailure> {
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
        // Workflow #191 H: the device session stays with the instance across the transfer; a
        // Ready transfer has no step in flight.
        self.end_lease_device_use(&from, prepared.from_connection_id(), LeaseDeviceEnd::Keep)?;
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
        self.deliver_claim_grant(&context, &committed)
            .map_err(RequestFailure::poison_without_terminal)?;
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
        let instance_id = queued.instance_id();
        let gate = self.routine_gate(instance_id)?;
        let order_lock = self.queue_order_lock(instance_id)?;
        let order = lock(&order_lock, "lock_instance_queue_order")?;
        let transfer = {
            let mut scheduler = lock(&self.scheduler, "prepare_idle_preemption")?;
            // Since the request became visible it may have been granted, expired or cancelled.
            let Some(active) = scheduler.active_lease(instance_id) else {
                return Ok(None);
            };
            scheduler
                .prepare_transfer_gated(
                    active.token(),
                    active.connection_id(),
                    LeaseTransferReason::Preempted,
                    None,
                    gate,
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
                if prepared.queued_request_id() != queued.request_id() {
                    return Ok(None);
                }
                if !self.capacity_allows_transfer(prepared.from_token())? {
                    drop(order);
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
            TransferPreparation::Deferred | TransferPreparation::NoCandidate => Ok(None),
        }
    }

    pub(super) fn expire_all_queued_runtime(&self) -> RuntimeHostResult<()> {
        let instance_ids = lock(&self.registered_instances, "read_instance_registry")?
            .keys()
            .copied()
            .collect::<Vec<_>>();
        for instance_id in instance_ids {
            self.expire_queued_for_instance(instance_id)
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
        let instance_id = self
            .resolve_instance(acquisition.instance_alias)?
            .instance_id();
        let instance_guard = self.instance_guard(instance_id)?;
        let admission = self.lock_admission(&instance_guard, instance_id)?;
        self.acquire_lease_guarded(acquisition, instance_id, &admission)
    }

    /// Workflow #369 W-1: an immediate try whose wait for the admission guard is bounded.
    /// Contention on the guard alone (a keyless observe or a monitor probe) is retried with
    /// `try_lock` until `retry` has passed; `None` when the guard stayed taken.
    pub(super) fn try_acquire_lease(
        &self,
        acquisition: RuntimeLeaseAcquisition<'_, '_>,
        retry: Duration,
    ) -> Result<Option<OperationSuccess>, RequestFailure> {
        let instance_id = self
            .resolve_instance(acquisition.instance_alias)?
            .instance_id();
        let instance_guard = self.instance_guard(instance_id)?;
        let started = Instant::now();
        let admission = loop {
            if let Some(admission) = self.try_lock_admission(&instance_guard, instance_id)? {
                break admission;
            }
            if started.elapsed() >= retry {
                return Ok(None);
            }
            thread::sleep(ADMISSION_CONTENTION_POLL_INTERVAL);
        };
        self.acquire_lease_guarded(acquisition, instance_id, &admission)
            .map(Some)
    }

    fn acquire_lease_guarded(
        &self,
        acquisition: RuntimeLeaseAcquisition<'_, '_>,
        instance_id: InstanceId,
        admission: &MutexGuard<'_, ()>,
    ) -> Result<OperationSuccess, RequestFailure> {
        let RuntimeLeaseAcquisition {
            request,
            request_id,
            instance_alias,
            holder_id,
            connection_id,
            run_links,
            lease_ttl_ms,
            kind,
        } = acquisition;
        let resolved = self.resolve_instance(instance_alias)?;
        if resolved.instance_id() != instance_id {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "runtime_instance_identity_mismatch",
                    "acquire_lease",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            ));
        }
        self.expire_instance_if_due(resolved.instance_id(), Some(admission))?;
        if kind.takes_admission_checks() {
            self.require_bound_endpoint(
                &resolved,
                self.events
                    .request_links(request, Some(resolved.instance_id()), None, None),
                EventAction::LeaseAcquire,
            )?;
        }
        let gate = self.routine_gate(instance_id)?;
        let order_lock = self.queue_order_lock(instance_id)?;
        let _order = lock(&order_lock, "lock_instance_queue_order")?;
        let preparation = {
            let mut scheduler = lock(&self.scheduler, "prepare_lease")?;
            let now_monotonic_ms = self.monotonic_ms()?;
            let lease_ttl_ms = lease_ttl_ms.unwrap_or(scheduler.config().lease_ttl_ms);
            scheduler.prepare_claim_acquire(
                ClaimRequest {
                    request_id,
                    instance_id: resolved.instance_id(),
                    holder_id,
                    connection_id,
                    kind,
                    priority: actingcommand_contract::LeasePriority::Normal,
                    lease_ttl_ms,
                },
                gate,
                now_monotonic_ms,
            )
        };
        let preparation = match preparation {
            Ok(preparation) => preparation,
            Err(error) => {
                self.append_lease_requested(request, &resolved)?;
                return Err(self.scheduler_denied(request, &resolved, None, error)?);
            }
        };
        if !preparation.is_existing() && kind.takes_admission_checks() {
            self.require_performance_control_lease(request, &resolved, instance_alias)?;
        }
        self.grant_prepared_lease(
            request,
            request_id,
            &resolved,
            preparation,
            run_links,
            kind_capacity(kind),
        )
    }

    /// A new Business lease is refused while the instance's performance-control directive asks
    /// for suspension or shutdown. Lower levels never block a lease; existing-lease replays,
    /// renewals and resource-close-only leases do not pass through this gate.
    fn require_performance_control_lease(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        resolved: &RegisteredInstance,
        instance_alias: &str,
    ) -> Result<(), RequestFailure> {
        let directive = lock(&self.performance_control, "gate_lease_performance_control")?
            .directive(instance_alias)?;
        if !(directive.suspend_requested || directive.shutdown_requested) {
            return Ok(());
        }
        self.append_lease_requested(request, resolved)?;
        Err(self.scheduler_denied_error(
            request,
            Some(resolved.instance_id()),
            None,
            resolved.audit_endpoint(),
            RuntimeHostError::request(
                "lease_refused_performance_control",
                "acquire_lease",
                RuntimeErrorCode::InvalidRequest,
            ),
        )?)
    }

    fn grant_prepared_lease(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        request_id: RequestId,
        resolved: &RegisteredInstance,
        preparation: LeasePreparation,
        run_links: Option<RuntimeRunLinks>,
        capacity_use: CapacityUse,
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
        self.grant_prepared_lease_with_links(resolved, preparation, links, capacity_use)
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
        let _admission = self.lock_admission(&instance_guard, token.instance_id())?;
        self.expire_queued_for_instance(token.instance_id())?;
        self.end_lease_device_use(token, connection_id, LeaseDeviceEnd::Keep)?;
        let gate = self.routine_gate(token.instance_id())?;
        let order_lock = self.queue_order_lock(token.instance_id())?;
        let _order = lock(&order_lock, "lock_instance_queue_order")?;
        let transfer = lock(&self.scheduler, "prepare_release_transfer")?
            .prepare_transfer_gated(
                token,
                connection_id,
                LeaseTransferReason::ExplicitRelease,
                Some(request_id),
                gate,
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
                if !waits_for_business_capacity(prepared.to_kind())
                    || self.capacity_allows_transfer(token)?
                {
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

    /// Lease validation on non-write paths: admits a lease-scoped request (lease renewal and
    /// release, input and application admission, contained-task and Lab-operation captures)
    /// by lease position before any witness exists. It authorizes no device write; each write
    /// runs under the `FencedWrite` that `begin_destructive_step` mints afterwards.
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
        let admission = self.lock_admission(&instance_guard, token.instance_id())?;
        self.transfer_preempted_while_guarded(token, connection_id, &admission)
    }

    pub(super) fn transfer_preempted_while_guarded(
        &self,
        token: &LeaseToken,
        connection_id: ConnectionId,
        _admission: &MutexGuard<'_, ()>,
    ) -> Result<bool, RequestFailure> {
        self.expire_queued_for_instance(token.instance_id())?;
        let gate = self.routine_gate(token.instance_id())?;
        let order_lock = self.queue_order_lock(token.instance_id())?;
        let order = lock(&order_lock, "lock_instance_queue_order")?;
        let transfer = lock(&self.scheduler, "prepare_preempted_transfer")?
            .prepare_transfer_gated(
                token,
                connection_id,
                LeaseTransferReason::Preempted,
                None,
                gate,
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
                    drop(order);
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
    ///
    /// Workflow #369 H-1 (review M-1): `trigger` is set only by the policy-run path. Its ladder
    /// is decided here, inside the lease end and before the release: an admitted ladder turns
    /// the end into a transfer of the key to the ladder's claim. Host package runs and
    /// `ensure_scheduled_policy_lease_released` pass `None`, so their failed end is an
    /// ordinary one that hands the key on through the queue.
    pub(super) fn cleanup_scheduled_failure_with_run_links(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        token: &LeaseToken,
        connection_id: ConnectionId,
        run_links: RuntimeRunLinks,
        trigger: Option<LadderTrigger>,
    ) -> RuntimeHostResult<()> {
        let Some(resolved) = self.cleanup_instance(token)? else {
            return Ok(());
        };
        #[cfg(test)]
        self.consume_contained_task_checkpoint_for_test(
            super::contained_task::ContainedTaskCheckpointPoint::FailedRunLeaseEnd,
            super::contained_task::ContainedTaskCheckpointIdentity::new(
                request.request_id(),
                token.instance_id(),
                token.lease_id(),
            ),
        )?;
        // The #670 test (f) crash point: after a failed run's terminal, before its lease end.
        #[cfg(test)]
        policy_crash_test_barrier("terminal_after_expiry");
        let instance_guard = self
            .instance_guard(token.instance_id())
            .map_err(|failure| *failure.error)?;
        let _admission = self.lock_admission(&instance_guard, token.instance_id())?;
        let ladder = match trigger {
            Some(trigger) => self.ladder_at_lease_end(request, &resolved, trigger)?,
            None => None,
        };
        // `Some` after this: an admitted ladder that found no key to hand on.
        let waiting = match ladder {
            Some(ladder) => match self.hand_off_to_ladder(
                token,
                connection_id,
                request,
                run_links,
                &resolved,
                ladder,
            )? {
                None => return Ok(()),
                Some(ladder) => Some(ladder),
            },
            None => None,
        };
        self.cleanup_token_guarded(
            token,
            connection_id,
            LeaseReleaseReason::BackendFailure,
            Some((request, run_links)),
            &resolved,
        )?;
        // No key to hand on (the lease already ended): the admitted ladder waits as a claim;
        // the guard's drop pumps it.
        match waiting {
            Some(ladder) => self.enqueue_ladder_claim(ladder),
            None => Ok(()),
        }
    }

    /// Workflow #369 H-1: turns a scheduled run's failed lease end into a transfer of the key
    /// to its admitted ladder. Under the queue-order lock: the ladder claim's own
    /// `lease.requested` and `scheduler.queued` (high, deadline `u64::MAX`, under the ladder's
    /// links; review L-2) and its registration, then the releaser's
    /// `lease.transition_intent`, the receiver's `lease.transition_intent`, `lease.transferred`
    /// and `lease.released` under the run's own links (C1, C9). The hand-off bypasses the queue
    /// and capacity (Q-3 (1), C13). The caller holds the admission guard. Returns the ladder
    /// back when there is no key to hand on (the lease already ended, a destructive step is
    /// open, or the gate closed since the admission).
    fn hand_off_to_ladder(
        &self,
        token: &LeaseToken,
        connection_id: ConnectionId,
        request: &ValidatedRuntimeRequest<'_>,
        run_links: RuntimeRunLinks,
        resolved: &RegisteredInstance,
        ladder: PendingRecoveryLadder,
    ) -> RuntimeHostResult<Option<PendingRecoveryLadder>> {
        let instance_id = token.instance_id();
        self.expire_queued_for_instance(instance_id)
            .map_err(|failure| *failure.error)?;
        self.end_lease_device_use(token, connection_id, LeaseDeviceEnd::Keep)
            .map_err(|failure| *failure.error)?;
        let claim_request = ladder.claim().clone();
        let validated_claim = claim_request.validate().map_err(|_| {
            RuntimeHostError::fatal(
                "recovery_ladder_claim_request_invalid",
                "hand_off_to_ladder",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        let claim_connection = ConnectionId::new(STARTUP_PACKAGE_CONNECTION_VALUE)
            .map_err(|error| RuntimeHostError::scheduler("build_ladder_connection", &error))?;
        let gate = self.routine_gate(instance_id)?;
        let order_lock = self
            .queue_order_lock(instance_id)
            .map_err(|failure| *failure.error)?;
        let _order = lock(&order_lock, "lock_instance_queue_order")?;
        let prepared = lock(&self.scheduler, "prepare_ladder_hand_off")?.prepare_hand_off(
            token,
            connection_id,
            LeaseTransferReason::BackendFailure,
            ClaimRequest {
                request_id: claim_request.request_id(),
                instance_id,
                holder_id: ladder.holder_id(),
                connection_id: claim_connection,
                kind: ClaimKind::Ladder,
                priority: actingcommand_contract::LeasePriority::High,
                lease_ttl_ms: LADDER_CLAIM_TTL_MS,
            },
            gate,
            self.monotonic_ms()?,
        );
        let prepared = match prepared {
            Ok(TransferPreparation::Ready(prepared)) => prepared,
            Ok(TransferPreparation::Deferred | TransferPreparation::NoCandidate)
            | Err(SchedulerError::LeaseMissing | SchedulerError::LeaseMismatch) => {
                return Ok(Some(ladder));
            }
            Err(error) => {
                return Err(RuntimeHostError::scheduler(
                    "prepare_ladder_hand_off",
                    &error,
                ));
            }
        };
        self.append_lease_requested(&validated_claim, resolved)
            .map_err(|failure| *failure.error)?;
        self.append_scheduler_queued_hand_off(&validated_claim, resolved)
            .map_err(|failure| *failure.error)?;
        #[cfg(test)]
        policy_crash_test_barrier("after_ladder_claim_queued_before_hand_off");
        let claim_request_id = validated_claim.request_id();
        self.register_queued_context(QueuedRequestContext {
            request: claim_request.clone(),
            instance: resolved.clone(),
            connection_id: claim_connection,
            grant: None,
        })
        .map_err(|failure| *failure.error)?;
        lock(&self.host_claim_work, "register_host_claim_work")?.insert(
            claim_request_id,
            HostClaimWork::RecoveryLadder(Box::new(ladder)),
        );
        self.cleanup_via_transfer(
            token,
            resolved,
            LeaseReleaseReason::BackendFailure,
            prepared,
            Some((request, run_links)),
        )?;
        Ok(None)
    }

    /// The ladder claim's `scheduler.queued` at a hand-off: high, first in line and with no
    /// deadline (Q-7), never requesting a preemption.
    fn append_scheduler_queued_hand_off(
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
            SchedulerPayloadDraft::queued(
                EventAction::ScheduleAdmit,
                actingcommand_contract::LeasePriority::High,
                1,
                actingcommand_scheduler::CLAIM_NO_DEADLINE_MONOTONIC_MS,
                false,
                audit_endpoint(resolved.audit_endpoint()),
            ),
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
        let Some(resolved) = self.cleanup_instance(token)? else {
            return Ok(());
        };
        let instance_guard = self
            .instance_guard(token.instance_id())
            .map_err(|failure| *failure.error)?;
        let _owned_admission = if admission.is_none() {
            Some(self.lock_admission(&instance_guard, token.instance_id())?)
        } else {
            None
        };
        self.cleanup_token_guarded(token, connection_id, reason, request_links, &resolved)
    }

    /// Workflow #369 Q-8: the sweep's cleanup of a lapsed lease takes the admission guard only
    /// with `try_lock`; while the holder is inside a device step it is retried at the next tick.
    /// Returns whether the lease was cleaned up.
    fn cleanup_expired_if_guard_free(
        &self,
        token: &LeaseToken,
        connection_id: ConnectionId,
    ) -> RuntimeHostResult<bool> {
        let Some(resolved) = self.cleanup_instance(token)? else {
            return Ok(true);
        };
        let instance_guard = self
            .instance_guard(token.instance_id())
            .map_err(|failure| *failure.error)?;
        let Some(_admission) = self.try_lock_admission(&instance_guard, token.instance_id())?
        else {
            return Ok(false);
        };
        self.cleanup_token_guarded(
            token,
            connection_id,
            LeaseReleaseReason::Expired,
            None,
            &resolved,
        )?;
        Ok(true)
    }

    /// The registered instance a cleanup acts on; `None` when the instance and the lease are
    /// both gone.
    fn cleanup_instance(
        &self,
        token: &LeaseToken,
    ) -> RuntimeHostResult<Option<RegisteredInstance>> {
        let resolved = lock(&self.registered_instances, "read_instance_registry")?
            .get(&token.instance_id())
            .cloned();
        if resolved.is_some() {
            return Ok(resolved);
        }
        let active = lock(&self.scheduler, "check_cleanup_lease")?
            .active_tokens()
            .into_iter()
            .any(|active| active == *token);
        if active {
            return Err(RuntimeHostError::fatal(
                "active_lease_instance_missing",
                "cleanup_runtime_connection",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        Ok(None)
    }

    /// A lease end under the instance's admission guard. Every end except `HostShutdown` hands
    /// the key on (Workflow #369 Q-4): a disconnect, an expiry and a backend failure transfer to
    /// the first eligible entry (with the run's links on the releaser's records, C1, C9); any
    /// other end releases and the guard's drop pumps. `HostShutdown` cancels the queue.
    fn cleanup_token_guarded(
        &self,
        token: &LeaseToken,
        connection_id: ConnectionId,
        reason: LeaseReleaseReason,
        request_links: Option<(&ValidatedRuntimeRequest<'_>, RuntimeRunLinks)>,
        resolved: &RegisteredInstance,
    ) -> RuntimeHostResult<()> {
        self.expire_queued_for_instance(token.instance_id())
            .map_err(|failure| *failure.error)?;
        // Workflow #191 H: every lease end keeps the instance's session, a HostShutdown too (the
        // Host close's second pass closes it under a dedicated close lease).
        let end = if reason == LeaseReleaseReason::Expired {
            LeaseDeviceEnd::Expiry
        } else {
            LeaseDeviceEnd::Keep
        };
        self.end_lease_device_use(token, connection_id, end)
            .map_err(|failure| *failure.error)?;
        let transfer_reason = match reason {
            LeaseReleaseReason::Disconnect => Some(LeaseTransferReason::Disconnect),
            LeaseReleaseReason::Expired => Some(LeaseTransferReason::Expired),
            LeaseReleaseReason::BackendFailure => Some(LeaseTransferReason::BackendFailure),
            LeaseReleaseReason::Explicit
            | LeaseReleaseReason::Preempted
            | LeaseReleaseReason::HostShutdown
            | LeaseReleaseReason::InstancePaused
            | LeaseReleaseReason::ConnectionPrepared => None,
        };
        let gate = self.routine_gate(token.instance_id())?;
        let order_lock = self
            .queue_order_lock(token.instance_id())
            .map_err(|failure| *failure.error)?;
        let _order = lock(&order_lock, "lock_instance_queue_order")?;
        if let Some(transfer_reason) = transfer_reason {
            let transfer = lock(&self.scheduler, "prepare_cleanup_transfer")?
                .prepare_transfer_gated(
                    token,
                    connection_id,
                    transfer_reason,
                    None,
                    gate,
                    self.monotonic_ms()?,
                );
            let transfer = match transfer {
                Ok(transfer) => transfer,
                // A backend-failure cleanup may meet a lease that already ended (it lapsed, or an
                // earlier cleanup released it): there is no key to hand on, and the release
                // below records the lease as already removed, as before #369.
                Err(SchedulerError::LeaseMissing | SchedulerError::LeaseMismatch)
                    if reason == LeaseReleaseReason::BackendFailure =>
                {
                    TransferPreparation::NoCandidate
                }
                Err(error) => {
                    return Err(RuntimeHostError::scheduler(
                        "prepare_cleanup_transfer",
                        &error,
                    ));
                }
            };
            match transfer {
                TransferPreparation::Ready(prepared) => {
                    if !waits_for_business_capacity(prepared.to_kind())
                        || self.capacity_allows_transfer(token)?
                    {
                        self.cleanup_via_transfer(
                            token,
                            resolved,
                            reason,
                            prepared,
                            request_links,
                        )?;
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
        if reason == LeaseReleaseReason::HostShutdown {
            self.cancel_instance_queue(token.instance_id(), DiagnosticCode::LeaseQueueDisconnected)
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
        let endpoint = resolved.audit_endpoint.clone();
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
                    .diagnostics()
                    .recorded_event()
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
        request_links: Option<(&ValidatedRuntimeRequest<'_>, RuntimeRunLinks)>,
    ) -> RuntimeHostResult<()> {
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
        #[cfg(test)]
        policy_crash_test_barrier("after_lease_transfer_before_release");
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
            if !self.cleanup_expired_if_guard_free(&token, connection_id)? {
                continue;
            }
            #[cfg(test)]
            self.record_completed_lease_expiry_for_test(&token)?;
        }
        self.pump_all()
    }

    pub(super) fn expire_instance_if_due(
        &self,
        instance_id: InstanceId,
        admission: Option<&MutexGuard<'_, ()>>,
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
            self.cleanup_token_inner(
                &token,
                connection_id,
                LeaseReleaseReason::Expired,
                None,
                admission,
            )
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
        // Workflow #191 ps2: a closed connection sends no further reset.
        self.forget_client_resets(connection_id)?;
        let queued_instances = lock(&self.scheduler, "list_connection_queues")?
            .queued_instance_ids_for_connection(connection_id);
        for instance_id in queued_instances {
            let order_lock = self
                .queue_order_lock(instance_id)
                .map_err(|failure| *failure.error)?;
            let order = lock(&order_lock, "lock_instance_queue_order")?;
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
            drop(order);
            self.pump(instance_id)?;
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
            .execution()?
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

/// Workflow #369: the one queue per instance (model-369-queue.md v3.1 Q-4 to Q-8).
impl HostShared {
    /// Takes the instance's admission guard, blocking, as a guard that pumps when dropped.
    pub(super) fn lock_admission<'a>(
        &'a self,
        mutex: &'a Mutex<()>,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<AdmissionGuard<'a>> {
        Ok(AdmissionGuard {
            guard: Some(lock(mutex, "lock_instance_admission")?),
            host: self,
            instance_id,
        })
    }

    /// Q-6, Q-8: takes the admission guard only if it is free. A poisoned guard is fatal.
    pub(super) fn try_lock_admission<'a>(
        &'a self,
        mutex: &'a Mutex<()>,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<Option<AdmissionGuard<'a>>> {
        match mutex.try_lock() {
            Ok(guard) => Ok(Some(AdmissionGuard {
                guard: Some(guard),
                host: self,
                instance_id,
            })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Poisoned(_)) => Err(lock_poison_error("try_lock_instance_admission")),
        }
    }

    pub(super) fn queue_order_lock(
        &self,
        instance_id: InstanceId,
    ) -> Result<Arc<Mutex<()>>, RequestFailure> {
        let mut locks = lock(&self.queue_order_locks, "read_instance_queue_order")?;
        Ok(Arc::clone(
            locks
                .entry(instance_id)
                .or_insert_with(|| Arc::new(Mutex::new(()))),
        ))
    }

    /// Q-3 (4): the instance's routine gate, read from the scheduling pauses now. It is read
    /// before the scheduler lock is taken.
    pub(super) fn routine_gate(&self, instance_id: InstanceId) -> RuntimeHostResult<ClaimGate> {
        let alias = lock(&self.registered_instances, "read_instance_registry")?
            .get(&instance_id)
            .map(|instance| instance.instance_alias.clone());
        let pauses = lock(&self.scheduling_pause, "read_routine_gate")?;
        let held = pauses.holds_global()
            || alias.is_some_and(|instance_alias| pauses.holds_instance(&instance_alias));
        Ok(if held {
            ClaimGate::RoutineHeld
        } else {
            ClaimGate::Open
        })
    }

    /// Q-4: grants the first eligible entry of a free instance, with `scheduler.admitted` +
    /// `lease.granted` under the entry's own request links. A pump that grants nothing writes
    /// nothing: an instance held, in takeover cooldown, without an eligible entry, short of
    /// business capacity for a claim that waits for it (`waits_for_business_capacity`), or whose
    /// admission guard is taken is left as it is (the guard's holder pumps when it drops it; the
    /// sweep pumps every tick).
    pub(super) fn pump(&self, instance_id: InstanceId) -> RuntimeHostResult<()> {
        if self.fatal.is_shutdown_requested() {
            return Ok(());
        }
        let gate = self.routine_gate(instance_id)?;
        let candidate = lock(&self.scheduler, "read_queue_grant_candidate")?.grant_candidate(
            instance_id,
            gate,
            self.monotonic_ms()?,
        );
        let Some(kind) = candidate else {
            return Ok(());
        };
        // Review L4: a kind that takes today's admission checks is not granted while the
        // instance's performance-control directive asks for suspension or shutdown.
        if kind.takes_admission_checks() && self.performance_control_withholds(instance_id)? {
            return Ok(());
        }
        if waits_for_business_capacity(kind) {
            match self.admit_capacity() {
                Ok(_) => {}
                Err(error) if error.is_fatal() => return Err(error),
                Err(_) => return Ok(()),
            }
        }
        let instance_guard = self
            .instance_guard(instance_id)
            .map_err(|failure| *failure.error)?;
        let _admission = match instance_guard.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::WouldBlock) => return Ok(()),
            Err(TryLockError::Poisoned(_)) => {
                return Err(lock_poison_error("pump_instance_queue"));
            }
        };
        let order_lock = self
            .queue_order_lock(instance_id)
            .map_err(|failure| *failure.error)?;
        let _order = lock(&order_lock, "lock_instance_queue_order")?;
        self.expire_queued_while_ordered(instance_id)
            .map_err(|failure| *failure.error)?;
        let prepared = lock(&self.scheduler, "prepare_queue_grant")?
            .prepare_queued_grant(instance_id, gate, self.monotonic_ms()?)
            .map_err(|error| RuntimeHostError::scheduler("prepare_queue_grant", &error))?;
        let Some(prepared) = prepared else {
            return Ok(());
        };
        let request_id = prepared.request_id();
        let context = self
            .read_queued_context(request_id)
            .map_err(|failure| *failure.error)?;
        let validated = context.request.validate().map_err(|_| {
            RuntimeHostError::fatal(
                "queued_request_context_invalid",
                "pump_instance_queue",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        self.append_scheduler_admitted(&validated, &context.instance, None)
            .map_err(|failure| *failure.error)?;
        let action_id = self.events.action_id()?;
        let links = self.events.request_links(
            &validated,
            Some(instance_id),
            Some(prepared.token().lease_id()),
            Some(action_id),
        );
        let token = prepared.token().clone();
        // Capacity was admitted above, so the grant itself writes no capacity refusal.
        self.grant_prepared_lease_with_links(
            &context.instance,
            LeasePreparation::New(prepared),
            links,
            CapacityUse::Drain,
        )
        .map_err(|failure| *failure.error)?;
        self.remove_queued_context(request_id, context.connection_id)
            .map_err(|failure| *failure.error)?;
        self.deliver_claim_grant(&context, &token)
    }

    /// Review L4: whether the instance's performance-control directive withholds a new lease of
    /// a kind that takes today's admission checks (read only; the pump writes nothing).
    fn performance_control_withholds(&self, instance_id: InstanceId) -> RuntimeHostResult<bool> {
        let Some(instance_alias) = lock(&self.registered_instances, "read_instance_registry")?
            .get(&instance_id)
            .map(|instance| instance.instance_alias.clone())
        else {
            return Ok(false);
        };
        let directive = lock(&self.performance_control, "gate_pump_performance_control")?
            .directive(&instance_alias)?;
        Ok(directive.suspend_requested || directive.shutdown_requested)
    }

    /// Q-6, W-2: tells a granted claim: a waiting claimant's slot gets the token, and a host
    /// claim's payload goes to its instance's worker with the key.
    fn deliver_claim_grant(
        &self,
        context: &QueuedRequestContext,
        token: &LeaseToken,
    ) -> RuntimeHostResult<()> {
        if let Some(grant) = &context.grant {
            grant.grant(token.clone())?;
        }
        let work = lock(&self.host_claim_work, "take_host_claim_work")?
            .remove(&context.request.request_id());
        if let Some(work) = work {
            self.post_host_claim(
                context.instance.instance_id(),
                work,
                HostKey {
                    request: context.request.clone(),
                    token: token.clone(),
                    connection_id: context.connection_id,
                },
            )?;
        }
        Ok(())
    }

    /// W-5: cancels one queued Runtime claim (`scheduler.denied lease.queue_cancelled`, its
    /// request links only, Warning) and forgets its payload. `false` when it is no longer
    /// queued (granted, or already cancelled by shutdown).
    pub(super) fn cancel_host_claim(
        &self,
        instance_id: InstanceId,
        request_id: RequestId,
        connection_id: ConnectionId,
    ) -> RuntimeHostResult<bool> {
        let order_lock = self
            .queue_order_lock(instance_id)
            .map_err(|failure| *failure.error)?;
        let _order = lock(&order_lock, "lock_instance_queue_order")?;
        let cancelled =
            lock(&self.scheduler, "cancel_host_claim")?.cancel_queued(request_id, connection_id);
        let cancelled = match cancelled {
            Ok(cancelled) => cancelled,
            Err(SchedulerError::QueueMissing) => return Ok(false),
            Err(error) => return Err(RuntimeHostError::scheduler("cancel_host_claim", &error)),
        };
        let context = self
            .take_queued_context(&cancelled)
            .map_err(|failure| *failure.error)?;
        self.append_queue_terminal(&context, DiagnosticCode::LeaseQueueCancelled)
            .map_err(|failure| *failure.error)?;
        lock(&self.host_claim_work, "forget_host_claim_work")?.remove(&request_id);
        Ok(true)
    }

    /// §5.2: an install drain cancels every waiting entry except a holding ladder's own
    /// continuation (`lease.queue_cancelled`); a holder finishes, or stops at its next holding
    /// check. Run by the sweep while the host drains.
    pub(super) fn cancel_queues_for_drain(&self) -> RuntimeHostResult<()> {
        let instance_ids = lock(&self.registered_instances, "read_instance_registry")?
            .keys()
            .copied()
            .collect::<Vec<_>>();
        for instance_id in instance_ids {
            let order_lock = self
                .queue_order_lock(instance_id)
                .map_err(|failure| *failure.error)?;
            let _order = lock(&order_lock, "lock_instance_queue_order")?;
            let removed = lock(&self.scheduler, "cancel_queues_for_drain")?
                .remove_queued_for_drain(instance_id)
                .map_err(|error| RuntimeHostError::scheduler("cancel_queues_for_drain", &error))?;
            for cancelled in removed {
                let context = self
                    .read_queued_context_for(cancelled.queued())
                    .map_err(|failure| *failure.error)?;
                self.finish_queue_terminal(
                    &context,
                    DiagnosticCode::LeaseQueueCancelled,
                    QueueTerminalOutcome::Cancelled { instance_id },
                )
                .map_err(|failure| *failure.error)?;
                lock(&self.host_claim_work, "forget_host_claim_work")?
                    .remove(&cancelled.queued().request_id());
            }
        }
        Ok(())
    }

    /// W-2: a stopping host cancels every queue (`lease.queue_disconnected`) before it joins
    /// the instance workers; pumping already stopped with the shutdown request.
    pub(super) fn cancel_all_queues_for_shutdown(&self) -> RuntimeHostResult<()> {
        let instance_ids = lock(&self.registered_instances, "read_instance_registry")?
            .keys()
            .copied()
            .collect::<Vec<_>>();
        for instance_id in instance_ids {
            let order_lock = self
                .queue_order_lock(instance_id)
                .map_err(|failure| *failure.error)?;
            let _order = lock(&order_lock, "lock_instance_queue_order")?;
            self.cancel_instance_queue(instance_id, DiagnosticCode::LeaseQueueDisconnected)
                .map_err(|failure| *failure.error)?;
        }
        Ok(())
    }

    /// Q-4: the sweep's backstop pump of every registered instance.
    pub(super) fn pump_all(&self) -> RuntimeHostResult<()> {
        let instance_ids = lock(&self.registered_instances, "read_instance_registry")?
            .keys()
            .copied()
            .collect::<Vec<_>>();
        for instance_id in instance_ids {
            self.pump(instance_id)?;
        }
        Ok(())
    }

    /// A pump that failed where its error cannot be returned (an admission guard's drop) marks
    /// the Runtime fatal. Every pump failure is an invariant or ledger failure.
    fn mark_pump_failure(&self, error: RuntimeHostError) {
        let error = if error.is_fatal() {
            error
        } else {
            RuntimeHostError::fatal(
                error.code(),
                "pump_instance_queue",
                RuntimeErrorCode::RuntimeFatal,
            )
        };
        if self.fatal.mark(error).is_err() {
            self.fatal.request_shutdown();
        }
    }

    fn read_queued_context(
        &self,
        request_id: RequestId,
    ) -> Result<QueuedRequestContext, RequestFailure> {
        lock(&self.queued_requests, "read_queued_request")?
            .get(&request_id)
            .cloned()
            .ok_or_else(|| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    "queued_request_context_missing",
                    "read_queued_request",
                    RuntimeErrorCode::RuntimeFatal,
                ))
            })
    }

    /// Q-2a, Q-6, Q-7: a Runtime-internal claim. Its checks follow its kind: emulator control,
    /// autostart, resume reconnect and self-check skip the bound-endpoint and
    /// performance-control checks and use drain capacity. On a free instance whose admission
    /// guard is free and where no eligible entry waits it is granted at once; otherwise it is
    /// queued with no deadline (never preempting) and made visible before it can be granted:
    /// `lease.requested` + `scheduler.queued` and its context, all under the queue-order lock.
    /// The grant then reaches the claimant through the returned slot. A business claim that
    /// capacity refuses now is queued, and the pump grants it once capacity recovers (review
    /// L5).
    pub(super) fn request_host_claim(
        &self,
        claim: HostClaim<'_>,
    ) -> Result<HostClaimAdmission, RequestFailure> {
        let validated = claim.request.validate().map_err(|_| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                "queued_request_context_invalid",
                "request_host_claim",
                RuntimeErrorCode::RuntimeFatal,
            ))
        })?;
        let resolved = self.resolve_instance(claim.instance_alias)?;
        let instance_id = resolved.instance_id();
        if claim.kind.takes_admission_checks() {
            self.require_bound_endpoint(
                &resolved,
                self.events
                    .request_links(&validated, Some(instance_id), None, None),
                EventAction::LeaseAcquire,
            )?;
            self.require_performance_control_lease(&validated, &resolved, claim.instance_alias)?;
        }
        let request = ClaimRequest {
            request_id: claim.request.request_id(),
            instance_id,
            holder_id: claim.holder_id,
            connection_id: claim.connection_id,
            kind: claim.kind,
            priority: claim.priority,
            lease_ttl_ms: claim.lease_ttl_ms,
        };
        let gate = self.routine_gate(instance_id)?;
        // Review L5: business capacity is asked before an immediate grant; a refusal queues the
        // claim (writing nothing) instead of failing it, and the pump grants it later. A startup
        // claim is not asked (review C-1).
        let capacity_free = !waits_for_business_capacity(claim.kind)
            || match self.admit_capacity() {
                Ok(_) => true,
                Err(error) if error.is_fatal() => {
                    return Err(RequestFailure::poison_without_terminal(error));
                }
                Err(_) => false,
            };
        let instance_guard = self.instance_guard(instance_id)?;
        let admission = self.try_lock_admission(&instance_guard, instance_id)?;
        let order_lock = self.queue_order_lock(instance_id)?;
        let order = lock(&order_lock, "lock_instance_queue_order")?;
        if admission.is_some() && capacity_free {
            let preparation = lock(&self.scheduler, "prepare_host_claim")?.prepare_claim_acquire(
                request,
                gate,
                self.monotonic_ms()?,
            );
            match preparation {
                Ok(preparation) => {
                    let token = preparation.token().clone();
                    // Capacity was admitted above, as the pump does.
                    self.grant_prepared_lease(
                        &validated,
                        request.request_id,
                        &resolved,
                        preparation,
                        None,
                        CapacityUse::Drain,
                    )?;
                    return Ok(HostClaimAdmission::Granted(token));
                }
                // Held, an entry ahead, or a takeover cooldown: the claim waits in the queue.
                Err(
                    SchedulerError::Busy { .. }
                    | SchedulerError::QueueAhead { .. }
                    | SchedulerError::Cooldown { .. },
                ) => {}
                Err(error) => {
                    self.append_lease_requested(&validated, &resolved)?;
                    return Err(self.scheduler_denied(&validated, &resolved, None, error)?);
                }
            }
        }
        let queued = lock(&self.scheduler, "enqueue_host_claim")?
            .enqueue_claim(request, self.monotonic_ms()?);
        let queued = match queued {
            Ok(queued) => queued,
            Err(error) => {
                self.append_lease_requested(&validated, &resolved)?;
                return Err(self.scheduler_denied(&validated, &resolved, None, error)?);
            }
        };
        self.append_lease_requested(&validated, &resolved)?;
        self.append_scheduler_queued(&validated, &resolved, &queued)?;
        let grant = Arc::new(ClaimGrantSlot::default());
        self.register_queued_context(QueuedRequestContext {
            request: claim.request.clone(),
            instance: resolved,
            connection_id: claim.connection_id,
            grant: Some(Arc::clone(&grant)),
        })?;
        drop(order);
        match admission {
            // Its drop pumps.
            Some(admission) => drop(admission),
            // Q-4: the guard's holder may have let it go before the entry was visible.
            None => self.pump(instance_id)?,
        }
        Ok(HostClaimAdmission::Queued {
            status: queued,
            grant,
        })
    }

    /// Q-5: renews a Runtime-held lease to `lease_ttl_ms` with today's renew set
    /// (`scheduler.admitted`, `lease.transition_intent`, `lease.renewed`, all Info) under the
    /// holder's request links. It runs under the queue-order lock and never takes the admission
    /// guard, which the holder may hold across a device step. A refusal (`lease_expired`,
    /// `lease_missing`) ends the hold loudly at its caller.
    pub(super) fn renew_held_lease(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        token: &LeaseToken,
        connection_id: ConnectionId,
        lease_ttl_ms: u64,
    ) -> Result<LeaseToken, RequestFailure> {
        let resolved = lock(&self.registered_instances, "read_instance_registry")?
            .get(&token.instance_id())
            .cloned()
            .ok_or_else(|| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    "active_lease_instance_missing",
                    "renew_held_lease",
                    RuntimeErrorCode::RuntimeFatal,
                ))
            })?;
        let order_lock = self.queue_order_lock(token.instance_id())?;
        let _order = lock(&order_lock, "lock_instance_queue_order")?;
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
                let renewed =
                    lock(&self.scheduler, "renew_held_lease").and_then(|mut scheduler| {
                        scheduler
                            .renew_with_ttl(
                                token,
                                connection_id,
                                lease_ttl_ms,
                                self.monotonic_ms()?,
                            )
                            .map_err(|error| {
                                RuntimeHostError::scheduler("renew_held_lease", &error)
                            })
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
        match result {
            Ok(receipt) => Ok(receipt.into_value()),
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

/// Workflow #369 W-1: how often a bounded admission retries a taken guard.
const ADMISSION_CONTENTION_POLL_INTERVAL: Duration = Duration::from_millis(5);

/// Whether business capacity holds a claim of `kind` back from its grant (the pump, the
/// immediate path and a transfer). Review C-1 (#666), model H-6: until S6b a startup claim is
/// not held back, so it never waits while capacity refuses; the startup run's own capacity
/// check (`prepare_package_run`) refuses it, records the refusal and releases the key, as a
/// refused startup run did before.
const fn waits_for_business_capacity(kind: ClaimKind) -> bool {
    !kind.skips_business_capacity() && !matches!(kind, ClaimKind::StartupPackage)
}

/// Q-2a: a grant to a drain-capacity kind never meets business capacity.
const fn kind_capacity(kind: ClaimKind) -> CapacityUse {
    if kind.skips_business_capacity() {
        CapacityUse::Drain
    } else {
        CapacityUse::Business
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
