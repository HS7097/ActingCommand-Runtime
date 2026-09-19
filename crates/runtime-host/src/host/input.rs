// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

impl HostShared {
    pub(super) fn input(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        token: &LeaseToken,
        action: &InputAction,
        connection_id: ConnectionId,
        execution_provenance: ExecutionBackendProvenance,
        context: RuntimeInputContext,
    ) -> Result<
        (
            OperationSuccess,
            Option<actingcommand_device::InputSelectionContext>,
        ),
        RequestFailure,
    > {
        let RuntimeInputContext {
            run_links,
            source_step_action_id,
            before_frame_id,
            input_frame,
            input_control,
        } = context;
        let (resolved, transferred) = {
            let instance_guard = self.instance_guard(token.instance_id())?;
            let admission = lock(&instance_guard, "lock_instance_admission")?;
            let resolved = self.validated_instance(request, token, connection_id)?;
            let transferred =
                self.transfer_preempted_while_guarded(token, connection_id, &admission)?;
            (resolved, transferred)
        };
        if resolved.provenance() != execution_provenance {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "execution_backend_provenance_mismatch",
                    "execute_input",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            ));
        }
        if transferred {
            return Err(self.scheduler_denied_error(
                request,
                Some(token.instance_id()),
                Some(token.lease_id()),
                resolved.audit_endpoint(),
                RuntimeHostError::scheduler(
                    "input_preempted_at_safe_boundary",
                    &SchedulerError::TransferNotSafe,
                ),
            )?);
        }
        let input_check = self
            .nemu_input_check(
                &resolved.instance_alias,
                token,
                connection_id,
                input_control,
                false,
            )
            .map_err(RequestFailure::poison_without_terminal)?;
        if input_check.is_some()
            && !matches!(
                action,
                InputAction::Tap { .. } | InputAction::SingleTouchDragWithVerticalBrakeV1 { .. }
            )
        {
            return Err(RequestFailure::request(
                RuntimeHostError::request(
                    "nemu_input_capability_unsupported",
                    "execute_input",
                    RuntimeErrorCode::InvalidRequest,
                ),
                RuntimeReceiptState::Denied,
                None,
            ));
        }
        if input_check.is_some() && input_frame.is_none() {
            return Err(RequestFailure::request(
                RuntimeHostError::request(
                    "nemu_input_frame_required",
                    "execute_input",
                    RuntimeErrorCode::InvalidRequest,
                ),
                RuntimeReceiptState::Denied,
                None,
            ));
        }
        if let Some(reference) = input_frame {
            self.execution
                .resolve_input_frame(&resolved.instance_alias, reference)
                .map_err(|error| {
                    RequestFailure::request(
                        RuntimeHostError::request(
                            "input_frame_unavailable",
                            "resolve_input_frame",
                            RuntimeErrorCode::InvalidRequest,
                        )
                        .with_native_detail(error.to_string()),
                        RuntimeReceiptState::Denied,
                        None,
                    )
                })?;
        }
        // Slice #316-B3: the foreground gate sits after the frame resolution and before the
        // input is prepared, so both touch backends pass the same check.
        self.require_foreground_application(request, token, &resolved, action, run_links)?;
        let before_frame_id = input_frame.map(|frame| frame.frame_id).or(before_frame_id);
        let prepared_action = self
            .execution
            .prepare_input(action.clone())
            .map_err(|error| {
                RequestFailure::poison_without_terminal(RuntimeHostError::execution(
                    "prepare_input",
                    &error,
                ))
            })?;
        let execution_plan = input_execution_plan_record(&prepared_action)
            .map_err(RequestFailure::poison_without_terminal)?;
        self.append_scheduler_admitted_for_token(request, token, resolved.audit_endpoint())?;
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
        let (source, module, success_effect, backend_failure_effect) = match execution_provenance {
            ExecutionBackendProvenance::PhysicalDevice => (
                EventSource::Device,
                OriginModule::DeviceProxy,
                DefiniteEffectDisposition::Performed,
                EffectDisposition::Indeterminate,
            ),
            ExecutionBackendProvenance::FixtureSimulation => (
                EventSource::Lab,
                OriginModule::Actinglab,
                DefiniteEffectDisposition::NotPerformed,
                EffectDisposition::NotPerformed,
            ),
        };
        let event_action = action.event_action();
        let intent_payload = InputPayloadDraft::intent_with_provenance(
            action.clone(),
            execution_plan,
            source_step_action_id,
            before_frame_id,
            execution_audit(execution_provenance, resolved.audit_endpoint()),
        );
        let intent = self
            .events
            .draft(
                EventSeverity::Info,
                source,
                module,
                EventActor::Runtime,
                links.clone(),
                intent_payload,
            )
            .and_then(|draft| self.events.sanitize(draft))
            .map_err(RequestFailure::poison_without_terminal)?;
        let plan = CriticalEventPlan::new(CriticalOperation::DeviceWrite, intent)
            .map_err(|_| RequestFailure::poison_without_terminal(critical_plan_error()))?;
        let endpoint = resolved.audit_endpoint.clone();
        let instance_alias = resolved.instance_alias.clone();
        let outcome_links = links.clone();
        let lifecycle_links = links.clone();
        let close_links = links.clone();
        let failure_links = links;
        let action_for_worker = prepared_action;
        let result = execute_critical(
            &self.ledger,
            self.events.fingerprinter(),
            plan,
            || {
                let destructive =
                    lock(&self.scheduler, "begin_destructive_input").and_then(|mut scheduler| {
                        scheduler
                            .begin_destructive_step(token, connection_id, self.monotonic_ms()?)
                            .map_err(|error| {
                                RuntimeHostError::scheduler("begin_destructive_input", &error)
                            })
                    });
                if let Err(error) = destructive {
                    return CriticalActionReport::Failed {
                        error: ActionFailure::scheduler(error),
                        effect: EffectDisposition::NotPerformed,
                    };
                }
                let registration = match self.mark_resources_in_use() {
                    Ok(registration) => registration,
                    Err(error) => {
                        return CriticalActionReport::Failed {
                            error: ActionFailure::poison(error),
                            effect: EffectDisposition::NotPerformed,
                        };
                    }
                };
                // Host-side span until the kernel-level backend span lands: it includes the
                // host→kernel channel round-trip on top of the backend write itself.
                let backend_started = Instant::now();
                let backend_result = self.execution.input_prepared_in_frame(
                    &instance_alias,
                    action_for_worker,
                    input_frame,
                    input_check,
                    registration,
                );
                let touch_response_us = performance::measured_microseconds(
                    actingcommand_execution_kernel::observe_instant_span(
                        backend_started,
                        Instant::now(),
                    ),
                );
                match backend_result {
                    Ok(outcome) => {
                        if let Some(recovery) = outcome.recovery
                            && let Err(error) = self.append_event_raw(
                                EventSeverity::Warning,
                                EventSource::Runtime,
                                OriginModule::Runtime,
                                EventActor::Runtime,
                                lifecycle_links.clone(),
                                RuntimePayloadDraft::adb_target_recovery(
                                    self.owner_epoch,
                                    recovery,
                                ),
                            )
                        {
                            return CriticalActionReport::Failed {
                                error: ActionFailure::poison(error),
                                effect: backend_failure_effect,
                            };
                        }
                        CriticalActionReport::Succeeded {
                            value: (outcome.selection, touch_response_us),
                            effect: success_effect,
                        }
                    }
                    Err(error) => {
                        if let Some(recovery) =
                            error.adb_recovery().filter(|report| report.recovered)
                            && let Err(failure) = self.append_event_raw(
                                EventSeverity::Warning,
                                EventSource::Runtime,
                                OriginModule::Runtime,
                                EventActor::Runtime,
                                lifecycle_links.clone(),
                                RuntimePayloadDraft::adb_target_recovery(
                                    self.owner_epoch,
                                    recovery.clone(),
                                ),
                            )
                        {
                            return CriticalActionReport::Failed {
                                error: ActionFailure::poison(failure),
                                effect: backend_failure_effect,
                            };
                        }
                        CriticalActionReport::Failed {
                            error: match self.finish_input_failure(
                                error,
                                token,
                                connection_id,
                                close_links,
                            ) {
                                Ok(error) => {
                                    let mut failure =
                                        ActionFailure::backend(RuntimeHostError::execution(
                                            "execute_input_backend",
                                            &error,
                                        ));
                                    failure.destructive_started = false;
                                    failure
                                }
                                Err(error) => ActionFailure::poison(error),
                            },
                            effect: backend_failure_effect,
                        }
                    }
                }
            },
            |(_, touch_response_us), effect| {
                self.events
                    .draft(
                        EventSeverity::Info,
                        source,
                        module,
                        EventActor::Runtime,
                        outcome_links,
                        InputPayloadDraft::committed_with_touch_response(
                            event_action,
                            effect.into(),
                            *touch_response_us,
                            execution_audit(execution_provenance, &endpoint),
                        ),
                    )
                    .map_err(|_| actingcommand_contract::SanitizationError::fingerprinter_failure())
            },
            |error, effect| {
                let audit = execution_audit(execution_provenance, &endpoint);
                let payload = InputPayloadDraft::failed_with_causes(
                    event_action,
                    error.diagnostic,
                    effect,
                    error.error.diagnostic_detail().cloned(),
                    error.error.cleanup_cause().cloned(),
                    audit,
                );
                self.events
                    .draft(
                        EventSeverity::Error,
                        source,
                        module,
                        EventActor::Runtime,
                        failure_links,
                        payload,
                    )
                    .map_err(|_| actingcommand_contract::SanitizationError::fingerprinter_failure())
            },
        );
        match result {
            Ok(receipt) => {
                self.finish_destructive_input(token, connection_id)?;
                self.transfer_preempted_if_ready(token, connection_id)?;
                self.observe_pipeline_event(receipt.outcome())
                    .map_err(RequestFailure::poison_without_terminal)?;
                let (selection, _) = receipt.value().clone();
                Ok((
                    OperationSuccess {
                        state: RuntimeReceiptState::Completed,
                        terminal: Some(terminal(receipt.outcome())),
                        result: RuntimeResult::InputCommitted { action_id },
                    },
                    selection,
                ))
            }
            Err(CriticalExecutionError::Action { error, outcome, .. }) => {
                self.record_required_failure(&error.error, &outcome, lifecycle_links)?;
                if self
                    .retain_unconfirmed_resources(&error.error, EventLinksDraft::default())
                    .map_err(RequestFailure::poison_without_terminal)?
                {
                    return Err(RequestFailure {
                        state: RuntimeReceiptState::Failed,
                        terminal: None,
                        error: Box::new(error.error.into_fatal()),
                        poison_runtime: true,
                        task_failure: error.task_failure.map(|evidence| *evidence),
                    });
                }
                if error.destructive_started {
                    self.finish_destructive_input(token, connection_id)?;
                }
                if error.transfer_after {
                    self.transfer_preempted_if_ready(token, connection_id)?;
                }
                let release_after = error.release_after;
                let failure = RequestFailure {
                    state: RuntimeReceiptState::Failed,
                    terminal: Some(terminal(&outcome)),
                    error: Box::new(error.error),
                    poison_runtime: error.poison_runtime,
                    task_failure: error.task_failure.map(|evidence| *evidence),
                };
                if release_after
                    && run_links.is_none()
                    && let Err(error) =
                        self.cleanup_token(token, connection_id, LeaseReleaseReason::BackendFailure)
                {
                    return Err(failure.replace_with_poison(error));
                }
                Err(failure)
            }
            Err(error) => Err(RequestFailure::poison_without_terminal(
                critical_execution_error(&error),
            )),
        }
    }

    /// Drives the assigned application under an existing lease. `run_links` carries the task
    /// and run ids when the action is a task package's `application` effect (slice #316-B3),
    /// so the `application.*` events sit inside the run's chain.
    pub(super) fn application_control(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        token: &LeaseToken,
        action: ApplicationLifecycleAction,
        connection_id: ConnectionId,
        run_links: Option<RuntimeRunLinks>,
    ) -> Result<OperationSuccess, RequestFailure> {
        self.require_physical_instance_id(token.instance_id())?;
        let (resolved, transferred) = {
            let instance_guard = self.instance_guard(token.instance_id())?;
            let admission = lock(&instance_guard, "lock_instance_admission")?;
            let resolved = self.validated_instance(request, token, connection_id)?;
            let transferred =
                self.transfer_preempted_while_guarded(token, connection_id, &admission)?;
            (resolved, transferred)
        };
        if transferred {
            return Err(self.scheduler_denied_error(
                request,
                Some(token.instance_id()),
                Some(token.lease_id()),
                resolved.audit_endpoint(),
                RuntimeHostError::scheduler(
                    "application_lifecycle_preempted_at_safe_boundary",
                    &SchedulerError::TransferNotSafe,
                ),
            )?);
        }
        self.append_scheduler_admitted_for_token(request, token, resolved.audit_endpoint())?;
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
        let event_action = action.event_action();
        let intent = self
            .events
            .draft(
                EventSeverity::Info,
                EventSource::Device,
                OriginModule::DeviceProxy,
                EventActor::Runtime,
                links.clone(),
                ApplicationPayloadDraft::intent(
                    event_action,
                    audit_endpoint(resolved.audit_endpoint()),
                ),
            )
            .and_then(|draft| self.events.sanitize(draft))
            .map_err(RequestFailure::poison_without_terminal)?;
        let plan = CriticalEventPlan::new(CriticalOperation::ApplicationLifecycle, intent)
            .map_err(|_| RequestFailure::poison_without_terminal(critical_plan_error()))?;
        let endpoint = resolved.audit_endpoint.clone();
        let instance_alias = resolved.instance_alias.clone();
        let outcome_links = links.clone();
        let failure_links = links;
        let result = execute_critical(
            &self.ledger,
            self.events.fingerprinter(),
            plan,
            || {
                let destructive = lock(&self.scheduler, "begin_destructive_application").and_then(
                    |mut scheduler| {
                        scheduler
                            .begin_destructive_step(token, connection_id, self.monotonic_ms()?)
                            .map_err(|error| {
                                RuntimeHostError::scheduler("begin_destructive_application", &error)
                            })
                    },
                );
                if let Err(error) = destructive {
                    return CriticalActionReport::Failed {
                        error: ActionFailure::scheduler(error),
                        effect: EffectDisposition::NotPerformed,
                    };
                }
                let registration = match self.mark_resources_in_use() {
                    Ok(registration) => registration,
                    Err(error) => {
                        return CriticalActionReport::Failed {
                            error: ActionFailure::poison(error),
                            effect: EffectDisposition::NotPerformed,
                        };
                    }
                };
                match self
                    .execution
                    .control_application_retained_with_registration_guard(
                        &instance_alias,
                        action,
                        registration,
                    ) {
                    Ok(()) => CriticalActionReport::Succeeded {
                        value: (),
                        effect: DefiniteEffectDisposition::Performed,
                    },
                    Err(error) => CriticalActionReport::Failed {
                        error: ActionFailure::backend(RuntimeHostError::execution(
                            "execute_application_backend",
                            &error,
                        )),
                        effect: EffectDisposition::Indeterminate,
                    },
                }
            },
            |_, effect| {
                self.events
                    .draft(
                        EventSeverity::Info,
                        EventSource::Device,
                        OriginModule::DeviceProxy,
                        EventActor::Runtime,
                        outcome_links,
                        ApplicationPayloadDraft::completed(
                            event_action,
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
                        EventSource::Device,
                        OriginModule::DeviceProxy,
                        EventActor::Runtime,
                        failure_links,
                        ApplicationPayloadDraft::failed(
                            event_action,
                            error.diagnostic,
                            effect,
                            audit_endpoint(&endpoint),
                        ),
                    )
                    .map_err(|_| actingcommand_contract::SanitizationError::fingerprinter_failure())
            },
        );
        match result {
            Ok(receipt) => {
                self.finish_destructive_input(token, connection_id)?;
                self.transfer_preempted_if_ready(token, connection_id)?;
                Ok(OperationSuccess {
                    state: RuntimeReceiptState::Completed,
                    terminal: Some(terminal(receipt.outcome())),
                    result: RuntimeResult::ApplicationLifecycleCompleted { action_id, action },
                })
            }
            Err(CriticalExecutionError::Action { error, outcome, .. }) => {
                let mut failure_links = self.events.request_links(
                    request,
                    Some(token.instance_id()),
                    Some(token.lease_id()),
                    Some(action_id),
                );
                if let Some(run_links) = run_links {
                    failure_links = run_links.apply(failure_links);
                }
                self.record_required_failure(&error.error, &outcome, failure_links)?;
                if self
                    .retain_unconfirmed_resources(&error.error, EventLinksDraft::default())
                    .map_err(RequestFailure::poison_without_terminal)?
                {
                    return Err(RequestFailure {
                        state: RuntimeReceiptState::Failed,
                        terminal: None,
                        error: Box::new(error.error.into_fatal()),
                        poison_runtime: true,
                        task_failure: None,
                    });
                }
                if error.destructive_started {
                    self.finish_destructive_input(token, connection_id)?;
                }
                if error.transfer_after {
                    self.transfer_preempted_if_ready(token, connection_id)?;
                }
                let release_after = error.release_after;
                let failure = RequestFailure {
                    state: RuntimeReceiptState::Failed,
                    terminal: Some(terminal(&outcome)),
                    error: Box::new(error.error),
                    poison_runtime: error.poison_runtime,
                    task_failure: None,
                };
                if release_after {
                    self.cleanup_token(token, connection_id, LeaseReleaseReason::BackendFailure)
                        .map_err(RequestFailure::poison_without_terminal)?;
                }
                Err(failure)
            }
            Err(error) => Err(RequestFailure::poison_without_terminal(
                critical_execution_error(&error),
            )),
        }
    }

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
        let executed = match self.application_control(request, &token, action, connection_id, None)
        {
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

#[derive(Clone, Default)]
pub(super) struct RuntimeInputContext {
    pub(super) run_links: Option<RuntimeRunLinks>,
    pub(super) source_step_action_id: Option<ActionId>,
    pub(super) before_frame_id: Option<actingcommand_contract::FrameId>,
    pub(super) input_frame: Option<actingcommand_contract::InputFrameReference>,
    pub(super) input_control: Option<Arc<ContainedRunControl>>,
}

fn input_execution_plan_record(
    prepared_action: &PreparedInputAction,
) -> RuntimeHostResult<Option<InputExecutionPlanRecord>> {
    let Some(plan) = prepared_action.segmented_swipe_plan() else {
        return Ok(None);
    };
    let events = plan
        .events()
        .iter()
        .map(|event| match event {
            SegmentedSwipeEvent::Down((x, y)) => InputExecutionPlanEvent::Down { x: *x, y: *y },
            SegmentedSwipeEvent::Move {
                point: (x, y),
                delay_before_ms,
            } => InputExecutionPlanEvent::Move {
                x: *x,
                y: *y,
                delay_before_ms: *delay_before_ms,
            },
            SegmentedSwipeEvent::Hold(duration_ms) => InputExecutionPlanEvent::Hold {
                duration_ms: *duration_ms,
            },
            SegmentedSwipeEvent::Up => InputExecutionPlanEvent::Up,
        })
        .collect();
    InputExecutionPlanRecord::new(events)
        .map(Some)
        .map_err(|_| {
            RuntimeHostError::fatal(
                "input_execution_plan_invalid",
                "prepare_input_execution_plan",
                RuntimeErrorCode::RuntimeFatal,
            )
        })
}

fn execution_audit(provenance: ExecutionBackendProvenance, endpoint: &str) -> AuditInput {
    match provenance {
        ExecutionBackendProvenance::PhysicalDevice => audit_endpoint(endpoint),
        ExecutionBackendProvenance::FixtureSimulation => AuditInput::new(),
    }
}
