// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

impl HostShared {
    pub(super) fn subscribe_events(
        &self,
        request: &RuntimeSubscriptionRequest,
    ) -> Result<OperationSuccess, RequestFailure> {
        let mut subscription = self
            .ledger
            .subscribe(request.cursor())
            .map_err(|_| RequestFailure::poison(ledger_error("subscribe_runtime_events"), None))?;
        let deadline = Instant::now()
            .checked_add(Duration::from_millis(request.wait_ms()))
            .ok_or_else(|| {
                RequestFailure::poison(protocol_error("subscribe_runtime_events"), None)
            })?;
        let mut events = Vec::with_capacity(usize::from(request.max_events()));
        let mut first_receive = true;
        let mut post_match_receive_budget = usize::from(request.max_events());
        let timed_out = loop {
            if events.len() == usize::from(request.max_events()) {
                break false;
            }
            let now = Instant::now();
            if events.is_empty() && !first_receive && now >= deadline {
                break true;
            }
            if !events.is_empty() {
                if post_match_receive_budget == 0 {
                    break false;
                }
                post_match_receive_budget -= 1;
            }
            let timeout = if events.is_empty() {
                deadline.saturating_duration_since(now)
            } else {
                Duration::ZERO
            };
            first_receive = false;
            match subscription.recv_timeout(timeout) {
                Ok(event) => {
                    let projected =
                        if request.query().view == Some(actingcommand_contract::LedgerView::Lab) {
                            // Lab membership needs the ledger's request/correlation/run context.
                            let mut query = request.query().clone();
                            query.view = None;
                            if project_subscription_event(
                                &event,
                                &query,
                                actingcommand_contract::ProjectionProfile::Concise,
                            )
                            .is_none()
                            {
                                continue;
                            }
                            query.view = request.query().view;
                            query.from_sequence =
                                Some(query.from_sequence.unwrap_or(0).max(event.sequence()));
                            query.to_sequence =
                                Some(query.to_sequence.unwrap_or(u64::MAX).min(event.sequence()));
                            let page = RuntimeEventQueryPageRequest::new(1, None)
                                .and_then(|page| page.at_snapshot(event.sequence()))
                                .map_err(|_| {
                                    RequestFailure::poison(
                                        protocol_error("project_runtime_subscription"),
                                        None,
                                    )
                                })?;
                            self.ledger
                                .project_view_page(query, request.profile(), page)
                                .map_err(|error| {
                                    if error.is_fatal() {
                                        RequestFailure::poison(
                                            ledger_error("project_runtime_subscription"),
                                            None,
                                        )
                                    } else {
                                        RequestFailure::request(
                                            RuntimeHostError::request(
                                                error.code(),
                                                "project_runtime_subscription",
                                                RuntimeErrorCode::ProtocolInvalid,
                                            ),
                                            RuntimeReceiptState::Denied,
                                            None,
                                        )
                                    }
                                })?
                                .events()
                                .first()
                                .cloned()
                        } else {
                            project_subscription_event(&event, request.query(), request.profile())
                        };
                    if let Some(projected) = projected {
                        events.push(projected);
                    }
                }
                Err(error) if error.code() == "subscription_timeout" => {
                    break events.is_empty();
                }
                Err(_) => {
                    return Err(RequestFailure::poison(
                        ledger_error("receive_runtime_subscription"),
                        None,
                    ));
                }
            }
        };
        let batch = RuntimeEventBatch::new(events, subscription.resume_cursor(), timed_out)
            .map_err(|_| {
                RequestFailure::poison(protocol_error("build_runtime_event_batch"), None)
            })?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::EventBatch { batch },
        })
    }

    pub(super) fn query_events(
        &self,
        query: &EventQuery,
        profile: actingcommand_contract::ProjectionProfile,
        request: &RuntimeEventQueryPageRequest,
    ) -> Result<OperationSuccess, RequestFailure> {
        let page = self
            .ledger
            .project_view_page(query.clone(), profile, request.clone())
            .map_err(|error| {
                if error.is_fatal() {
                    RequestFailure::poison(ledger_error("query_runtime_events"), None)
                } else {
                    RequestFailure::request(
                        RuntimeHostError::request(
                            error.code(),
                            "query_runtime_events",
                            RuntimeErrorCode::ProtocolInvalid,
                        ),
                        RuntimeReceiptState::Denied,
                        None,
                    )
                }
            })?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::EventPage { page },
        })
    }

    pub(super) fn control_plane_status(
        &self,
        validated: &ValidatedRuntimeRequest<'_>,
    ) -> Result<OperationSuccess, RequestFailure> {
        let (state, source) = self.observe_runtime_state(validated, || {
            Ok(actingcommand_contract::RuntimeObservedState::ControlPlane {
                status: self.control_plane_status_projection()?,
            })
        })?;
        let actingcommand_contract::RuntimeObservedState::ControlPlane { status } = state else {
            return Err(RequestFailure::poison_without_terminal(ledger_error(
                "runtime_state_observation_kind",
            )));
        };
        let status = status.with_source(source).map_err(|_| {
            RequestFailure::poison_without_terminal(ledger_error("runtime_state_source"))
        })?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::Status { status },
        })
    }

    pub(super) fn observe_runtime_state(
        &self,
        validated: &ValidatedRuntimeRequest<'_>,
        sample: impl FnOnce() -> Result<actingcommand_contract::RuntimeObservedState, RequestFailure>,
    ) -> Result<
        (
            actingcommand_contract::RuntimeObservedState,
            actingcommand_contract::RuntimeStateSource,
        ),
        RequestFailure,
    > {
        let sampled_started_at_unix_ms = self
            .clock
            .sample()
            .map_err(RequestFailure::poison_without_terminal)?
            .unix_ms;
        let state = sample()?;
        let sampled_completed_at_unix_ms = self
            .clock
            .sample()
            .map_err(RequestFailure::poison_without_terminal)?
            .unix_ms;
        let observed = self.append_event(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            self.events.request_links(validated, None, None, None),
            CommandPayloadDraft::validated_runtime_state(
                EventAction::RuntimeAction,
                EffectDisposition::NotPerformed,
                actingcommand_contract::RuntimeStateFact::Observed {
                    sampled_started_at_unix_ms,
                    sampled_completed_at_unix_ms,
                    state,
                },
                AuditInput::new(),
            ),
        )?;
        let Some(actingcommand_contract::RuntimeStateFact::Observed {
            sampled_started_at_unix_ms,
            sampled_completed_at_unix_ms,
            state,
        }) = observed.payload().runtime_state()
        else {
            return Err(RequestFailure::poison_without_terminal(ledger_error(
                "runtime_state_observation_missing",
            )));
        };
        let source = actingcommand_contract::RuntimeStateSource {
            event_id: *observed.event_id(),
            sequence: observed.sequence(),
            sampled_started_at_unix_ms: *sampled_started_at_unix_ms,
            sampled_completed_at_unix_ms: *sampled_completed_at_unix_ms,
        };
        source.validate().map_err(|_| {
            RequestFailure::poison_without_terminal(ledger_error("runtime_state_source"))
        })?;
        let state = state.clone();
        Ok((state, source))
    }

    pub(super) fn control_plane_status_projection(
        &self,
    ) -> Result<RuntimeControlPlaneStatus, RequestFailure> {
        let now = self
            .monotonic_ms()
            .map_err(RequestFailure::poison_without_terminal)?;
        let instances = lock(&self.registered_instances, "read_runtime_status_registry")?
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let scheduler = lock(&self.scheduler, "read_runtime_status_scheduler")?;
        let mut projected = Vec::with_capacity(instances.len());
        for instance in instances {
            let instance_id = instance.instance_id();
            let active = scheduler.active_lease(instance_id);
            let queued_request_count =
                u32::try_from(scheduler.queued_count(instance_id)).map_err(|_| {
                    RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                        "runtime_status_queue_count_overflow",
                        "project_runtime_control_plane_status",
                        RuntimeErrorCode::RuntimeFatal,
                    ))
                })?;
            projected.push(
                RuntimeInstanceStatus::new(
                    instance.instance_alias,
                    instance_id,
                    active.is_some(),
                    queued_request_count,
                    scheduler.cooldown_active(instance_id, now),
                    active
                        .as_ref()
                        .is_some_and(|lease| lease.destructive_step_active()),
                    active
                        .as_ref()
                        .is_some_and(|lease| lease.preempt_requested()),
                )
                .map_err(|_| {
                    RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                        "runtime_status_projection_invalid",
                        "project_runtime_control_plane_status",
                        RuntimeErrorCode::RuntimeFatal,
                    ))
                })?,
            );
        }
        let status = RuntimeControlPlaneStatus::new(self.owner_epoch, projected).map_err(|_| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                "runtime_status_projection_invalid",
                "project_runtime_control_plane_status",
                RuntimeErrorCode::RuntimeFatal,
            ))
        })?;
        Ok(status)
    }

    pub(super) fn monitor_status(
        &self,
        validated: &ValidatedRuntimeRequest<'_>,
    ) -> Result<OperationSuccess, RequestFailure> {
        let (state, source) = self.observe_runtime_state(validated, || {
            let status = lock(&self.monitor_registry, "read_monitor_registry")?
                .status(self.owner_epoch)
                .map_err(RequestFailure::poison_without_terminal)?;
            Ok(actingcommand_contract::RuntimeObservedState::Monitor { status })
        })?;
        let actingcommand_contract::RuntimeObservedState::Monitor { status } = state else {
            return Err(RequestFailure::poison_without_terminal(ledger_error(
                "monitor_state_observation_kind",
            )));
        };
        let status = status.with_source(source).map_err(|_| {
            RequestFailure::poison_without_terminal(ledger_error("monitor_state_source"))
        })?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::MonitorStatus { status },
        })
    }
}
