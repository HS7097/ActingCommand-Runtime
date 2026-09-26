// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::project_interface::{
    MAX_PROJECT_DIAGNOSTICS, ProjectDiagnosticProjection, ProjectInterfaceProjection,
    retain_recent_diagnostics,
};

impl HostShared {
    pub(super) fn project_interface(
        &self,
        validated: &ValidatedRuntimeRequest<'_>,
        request: &ProjectInterfaceRequest,
    ) -> Result<OperationSuccess, RequestFailure> {
        request.negotiate().map_err(|error| {
            project_interface_failure(RuntimeHostError::request(
                error.code(),
                "negotiate_project_interface",
                RuntimeErrorCode::ProtocolInvalid,
            ))
        })?;
        let current_ledger_position = self.ledger.latest_sequence().map_err(|_| {
            RequestFailure::poison_without_terminal(ledger_error("project_runtime_position"))
        })?;
        let decision_page = request
            .decision_page()
            .cloned()
            .unwrap_or_else(ProjectDecisionPageRequest::default);
        let ledger_position = decision_page
            .cursor()
            .map_or(current_ledger_position, |cursor| {
                cursor.snapshot_ledger_position()
            });
        if ledger_position == 0 || ledger_position > current_ledger_position {
            return Err(project_interface_failure(RuntimeHostError::request(
                "project_decision_cursor_invalid",
                "project_runtime_interface",
                RuntimeErrorCode::ProtocolInvalid,
            )));
        }
        // The live fact store answers the position it has replayed exactly through (Workflow
        // #317 item E); any other position replays the ledger.
        let live_facts =
            lock(&self.facts, "project_runtime_facts")?.active_records_if_at(ledger_position);
        let facts = match live_facts {
            Some(facts) => facts,
            None => InstanceFactStore::active_records_at(&self.ledger, ledger_position)
                .map_err(RequestFailure::poison_without_terminal)?,
        };
        let approvals = lock(&self.approval_records, "project_runtime_approvals")?
            .records_at(&self.ledger, Arc::clone(&self.state), ledger_position)
            .map_err(RequestFailure::poison_without_terminal)?;
        let (catalog, decisions) = {
            let mut policy = lock(&self.policy, "project_runtime_policy")?;
            policy
                .refresh_dispatches(&self.ledger)
                .map_err(RequestFailure::poison_without_terminal)?;
            (
                policy
                    .active_loaded_at(&self.ledger, ledger_position)
                    .map_err(project_interface_failure)?,
                policy.project_dispatches(current_ledger_position, &decision_page)?,
            )
        };
        if decisions.snapshot_ledger_position != ledger_position {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "project_snapshot_position_mismatch",
                    "project_runtime_interface",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            ));
        }
        let diagnostics = self.recent_project_diagnostics(ledger_position)?;
        let (state, source) = self.observe_runtime_state(validated, || {
            let status = self.control_plane_status_projection()?;
            let fatal = self
                .fatal
                .current()
                .map_err(RequestFailure::poison_without_terminal)?
                .is_some();
            Ok(actingcommand_contract::RuntimeObservedState::ProjectCurrent { status, fatal })
        })?;
        let actingcommand_contract::RuntimeObservedState::ProjectCurrent { status, fatal } = state
        else {
            return Err(RequestFailure::poison_without_terminal(ledger_error(
                "project_state_observation_kind",
            )));
        };
        let current_ledger_position = source.sequence;
        let status = status.with_source(source).map_err(|_| {
            RequestFailure::poison_without_terminal(ledger_error("project_state_source"))
        })?;
        let response = ProjectInterfaceProjection {
            ledger_position,
            current_ledger_position,
            catalog,
            instances: status,
            facts,
            decisions,
            approvals,
            diagnostics: retain_recent_diagnostics(diagnostics),
            fatal,
        }
        .into_response(request)
        .map_err(project_interface_failure)?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::ProjectInterface {
                response: Box::new(response),
            },
        })
    }

    /// The most recent diagnostics at or before `ledger_position`, in ledger order. They are
    /// read backwards in doubling sequence windows and the read stops once the projection cap
    /// is filled (Workflow #317 item C); fewer diagnostics than the cap read every window.
    fn recent_project_diagnostics(
        &self,
        ledger_position: u64,
    ) -> Result<Vec<ProjectDiagnosticProjection>, RequestFailure> {
        let query = EventQuery {
            to_sequence: Some(ledger_position),
            minimum_severity: Some(EventSeverity::Warning),
            ..EventQuery::default()
        };
        let mut recent = VecDeque::new();
        let mut upper = ledger_position;
        let mut window = MAX_PROJECT_DIAGNOSTICS as u64;
        while upper > 0 && recent.len() < MAX_PROJECT_DIAGNOSTICS {
            let lower = upper.saturating_sub(window);
            let mut older = Vec::new();
            let mut after = lower;
            loop {
                let page = self
                    .ledger
                    .query_page(query.clone(), after, upper, MAX_PROJECT_DIAGNOSTICS)
                    .map_err(|_| {
                        RequestFailure::poison_without_terminal(ledger_error(
                            "project_runtime_diagnostics",
                        ))
                    })?;
                let exhausted = page.len() < MAX_PROJECT_DIAGNOSTICS;
                if let Some(last) = page.last() {
                    after = last.sequence();
                }
                older.extend(page.into_iter().map(|event| ProjectDiagnosticProjection {
                    sequence: event.sequence(),
                    timestamp_unix_ms: event.timestamp_unix_ms(),
                    severity: event.severity(),
                    event_type: event.event_type(),
                }));
                if older.len() > MAX_PROJECT_DIAGNOSTICS {
                    older.drain(..older.len() - MAX_PROJECT_DIAGNOSTICS);
                }
                if exhausted {
                    break;
                }
            }
            for diagnostic in older.into_iter().rev() {
                recent.push_front(diagnostic);
            }
            upper = lower;
            window = window.saturating_mul(2);
        }
        Ok(retain_recent_diagnostics(recent.into()))
    }

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
        // The game of each configured policy identity (Workflow #308 slice 4a-2), read and
        // released before the registry lock; a host without policy inputs names no game.
        let configured_games = lock(&self.policy_inputs, "read_runtime_status_policy_games")?
            .as_ref()
            .map(|inputs| {
                inputs
                    .instance_identities()
                    .map(|identity| (identity.instance_id.to_owned(), identity.game_id.to_owned()))
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default();
        // Resolve only metadata before taking the scheduler lock. Identity checks and the
        // registered instance set are shared with device-facing admission; they run under
        // the registry lock so an endpoint rebinding is never observed half-applied.
        let instances = lock(&self.registered_instances, "read_runtime_status_registry")?
            .values()
            .map(|instance| {
                let resolved = self.resolve_registered_backend(instance)?;
                Ok((instance.clone(), resolved))
            })
            .collect::<Result<Vec<_>, RequestFailure>>()?;
        let scheduler = lock(&self.scheduler, "read_runtime_status_scheduler")?;
        let mut projected = Vec::with_capacity(instances.len());
        for (instance, resolved) in instances {
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
            // The port identifies the instance in the ledger only when it was configured as
            // HOST:PORT; a serial-configured target, a pending discovery binding or no ADB
            // endpoint carries no port.
            let adb_port = instance
                .bound_adb_endpoint()
                .filter(|endpoint| !endpoint.serial_configured())
                .map(ResolvedAdbEndpoint::port);
            let resource_package = self
                .resource_packages
                .get(&instance.instance_alias)
                .cloned();
            let game_id = configured_games.get(&instance.instance_alias).cloned();
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
                })?
                .with_backend_metadata(resolved.provenance(), resolved.capabilities().cloned())
                .with_adb_port(adb_port)
                .with_resource_package(resource_package)
                .with_game_id(game_id)
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

fn project_interface_failure(error: RuntimeHostError) -> RequestFailure {
    if error.is_fatal() {
        RequestFailure::poison_without_terminal(error)
    } else {
        RequestFailure::request(error, RuntimeReceiptState::Denied, None)
    }
}
