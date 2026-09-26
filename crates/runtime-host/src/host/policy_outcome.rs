// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[derive(Clone, Copy)]
pub(super) enum PolicyOutcomeCacheUpdate<'a> {
    #[cfg(test)]
    Retain,
    Commit(Option<&'a AuthoritativeSchedulingOutcome>),
    Clear(&'a PolicyRunContext),
}

impl HostShared {
    pub(super) fn reconcile_pending_policy_settlements(&self) -> RuntimeHostResult<()> {
        let result: RuntimeHostResult<()> = (|| {
            let _gate = lock(&self.policy_outcome_gate, "reconcile_policy_settlements")?;
            let mut policy = lock(&self.policy, "reconcile_policy_settlements")?;
            let missing_outcomes = policy
                .pending_dispatch_outcomes()
                .into_iter()
                .collect::<BTreeSet<_>>();
            let mut pending = policy.pending_dispatch_completions();
            pending.extend(missing_outcomes.iter().cloned());
            if pending.is_empty() {
                return Ok(());
            }
            let eligible = self.inactive_policy_settlements(&pending, &missing_outcomes)?;
            if eligible.is_empty() {
                return Ok(());
            }
            reconcile_scheduled_policy_outcomes_for(
                &mut policy,
                &self.ledger,
                eligible
                    .iter()
                    .filter(|id| missing_outcomes.contains(*id))
                    .cloned()
                    .collect(),
            )?;
            let pending_completions = policy
                .pending_dispatch_completions()
                .into_iter()
                .collect::<BTreeSet<_>>();
            let reconciled = eligible.clone();
            let mut settled = false;
            for decision_id in eligible {
                if pending_completions.contains(&decision_id) {
                    let execution = policy.execution_data(&decision_id)?;
                    policy.completion_data(&decision_id)?;
                    let completion = self
                        .ledger
                        .reconcile_scheduled_policy_settlement(execution)
                        .map_err(|_| ledger_error("reconcile_policy_settlements"))?;
                    policy.complete_dispatch(&decision_id, &completion)?;
                }
                if !policy.dispatch_needs_completion(&decision_id)? {
                    lock(
                        &self.policy_dispatch_clocks,
                        "clear_reconciled_policy_clock",
                    )?
                    .remove(&decision_id);
                    settled = true;
                }
            }
            if settled {
                *lock(
                    &self.authoritative_policy_outcomes,
                    "recover_online_policy_outcomes",
                )? = recover_authoritative_policy_outcomes(&policy, &self.ledger)?;
            }
            // Reconciled settlements carry their four settlement facts too (Workflow #308
            // slice 5b).
            let mut settlements = Vec::new();
            for decision_id in &reconciled {
                if let Some(settlement) = policy.latest_settlement_for_decision(decision_id)? {
                    settlements.push(settlement);
                }
            }
            drop(policy);
            for settlement in &settlements {
                self.record_policy_settlement_facts(settlement)?;
            }
            Ok(())
        })();
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.fatal.mark(error.clone())?;
        }
        result
    }

    // policy_outcome_gate excludes a context committing its outcome while these
    // exact ledger links are checked against the existing execution owners.
    fn inactive_policy_settlements(
        &self,
        pending: &[String],
        missing_outcomes: &BTreeSet<String>,
    ) -> RuntimeHostResult<Vec<String>> {
        // Every read below is pinned to one position. Past the dispatch intents, each pending
        // decision is read through its own run links (Workflow #317 item B), never the whole
        // ledger; every original predicate still selects the events it counts.
        let through = self
            .ledger
            .latest_sequence()
            .map_err(|_| ledger_error("select_policy_settlements"))?;
        let dispatch_intents = self
            .ledger
            .query(EventQuery {
                to_sequence: Some(through),
                event_type: Some(EventType::PolicyDispatchIntent),
                ..EventQuery::default()
            })
            .map_err(|_| ledger_error("select_policy_settlements"))?;
        let leases = lock(&self.scheduler, "select_policy_settlements")?.active_tokens();
        let runs = lock(&self.contained_runs, "select_policy_settlements")?;
        let mut eligible = Vec::new();
        for decision_id in pending {
            let intents = dispatch_intents
                .iter()
                .filter(|event| {
                    matches!(
                        event.payload(),
                        EventPayload::Policy(PolicyPayload::DispatchIntent(payload))
                            if payload.decision_id() == decision_id.as_str()
                    )
                })
                .collect::<Vec<_>>();
            let [intent] = intents.as_slice() else {
                return Err(policy_admission_fatal(
                    "policy_dispatch_intent_not_unique",
                    "select_policy_settlements",
                ));
            };
            let links = intent.links();
            if links.task_id().is_none() || links.run_id().is_none() {
                continue;
            }
            if links.instance_id().is_none()
                || links.request_id().is_none()
                || links.correlation_id().is_none()
            {
                return Err(policy_admission_fatal(
                    "policy_run_identity_missing",
                    "select_policy_settlements",
                ));
            }
            // The contained request is minted after admission. Its existing owner
            // retains the instance even before its first task event is appended.
            if runs
                .values()
                .any(|run| Some(run.instance_id) == links.instance_id().copied())
            {
                continue;
            }
            let run_links = EventQuery {
                to_sequence: Some(through),
                instance_id: links.instance_id().copied(),
                correlation_id: links.correlation_id().copied(),
                task_id: links.task_id().copied(),
                run_id: links.run_id().copied(),
                ..EventQuery::default()
            };
            let grants = linked_policy_run_events(
                &self.ledger,
                EventQuery {
                    event_type: Some(EventType::LeaseGranted),
                    request_id: links.request_id().copied(),
                    ..run_links.clone()
                },
                through,
                2,
                "select_policy_settlements",
                |event| {
                    event.event_type() == EventType::LeaseGranted
                        && event.links().instance_id() == links.instance_id()
                        && event.links().request_id() == links.request_id()
                        && event.links().correlation_id() == links.correlation_id()
                        && event.links().task_id() == links.task_id()
                        && event.links().run_id() == links.run_id()
                },
            )?;
            let grant = match grants.as_slice() {
                [] => continue,
                [grant] => grant,
                _ => {
                    return Err(policy_admission_fatal(
                        "policy_run_lease_fact_not_unique",
                        "select_policy_settlements",
                    ));
                }
            };
            let Some(lease_id) = grant.links().lease_id() else {
                return Err(policy_admission_fatal(
                    "policy_run_identity_missing",
                    "select_policy_settlements",
                ));
            };
            if leases.iter().any(|token| token.lease_id() == *lease_id) {
                continue;
            }
            let lease_links = EventQuery {
                lease_id: Some(*lease_id),
                ..run_links.clone()
            };
            let releases = linked_policy_run_events(
                &self.ledger,
                EventQuery {
                    event_type: Some(EventType::LeaseReleased),
                    ..lease_links.clone()
                },
                through,
                2,
                "select_policy_settlements",
                |event| {
                    event.event_type() == EventType::LeaseReleased
                        && event.links().instance_id() == links.instance_id()
                        && event.links().request_id().is_some()
                        && event.links().correlation_id() == links.correlation_id()
                        && event.links().task_id() == links.task_id()
                        && event.links().run_id() == links.run_id()
                        && event.links().lease_id() == Some(lease_id)
                },
            )?
            .len();
            if releases > 1 {
                return Err(policy_admission_fatal(
                    "policy_run_release_fact_not_unique",
                    "select_policy_settlements",
                ));
            }
            let mut terminals = 0;
            for terminal_type in [
                EventType::TaskCompleted,
                EventType::TaskFailed,
                EventType::TaskCancelled,
            ] {
                if terminals > 1 {
                    break;
                }
                terminals += linked_policy_run_events(
                    &self.ledger,
                    EventQuery {
                        event_type: Some(terminal_type),
                        ..lease_links.clone()
                    },
                    through,
                    2 - terminals,
                    "select_policy_settlements",
                    |event| {
                        matches!(
                            event.event_type(),
                            EventType::TaskCompleted
                                | EventType::TaskFailed
                                | EventType::TaskCancelled
                        ) && event.links().instance_id() == links.instance_id()
                            && event.links().request_id().is_some()
                            && event.links().correlation_id() == links.correlation_id()
                            && event.links().task_id() == links.task_id()
                            && event.links().run_id() == links.run_id()
                            && event.links().lease_id() == Some(lease_id)
                    },
                )?
                .len();
            }
            if terminals > 1 {
                return Err(policy_admission_fatal(
                    "policy_run_terminal_not_unique",
                    "select_policy_settlements",
                ));
            }
            if terminals == 0 {
                // The accepted recovery consumer can settle a released admission
                // without a task terminal only when no effect was started.
                if !missing_outcomes.contains(decision_id)
                    || linked_policy_run_events(
                        &self.ledger,
                        EventQuery {
                            event_type: Some(EventType::LeaseReleased),
                            request_id: links.request_id().copied(),
                            ..lease_links.clone()
                        },
                        through,
                        1,
                        "select_policy_settlements",
                        |event| scheduled_admission_release_matches(event, intent, lease_id),
                    )?
                    .is_empty()
                    || policy_run_effect_started(&self.ledger, links, through)?
                {
                    continue;
                }
            }
            if releases == 1 {
                eligible.push(decision_id.clone());
            }
        }
        Ok(eligible)
    }

    pub(super) fn complete_scheduled_policy_run(
        &self,
        context: &PolicyRunContext,
        receipt: &RuntimeReceipt,
    ) -> RuntimeHostResult<(
        PolicyExecutionEventData,
        Option<SchedulingOutcomeProjection>,
    )> {
        let _gate = lock(&self.policy_outcome_gate, "complete_scheduled_policy_run")?;
        receipt.validate().map_err(|_| {
            RuntimeHostError::fatal(
                "policy_run_receipt_invalid",
                "complete_scheduled_policy_run",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        let terminal = receipt.terminal().ok_or_else(|| {
            RuntimeHostError::fatal(
                "policy_run_receipt_terminal_missing",
                "complete_scheduled_policy_run",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        let Some(RuntimeResult::ContainedTaskCompleted {
            run_id,
            task_id,
            outcome,
            ..
        }) = receipt.result()
        else {
            return Err(RuntimeHostError::fatal(
                "policy_run_receipt_result_invalid",
                "complete_scheduled_policy_run",
                RuntimeErrorCode::RuntimeFatal,
            ));
        };
        if receipt.state() != RuntimeReceiptState::Completed
            || receipt.correlation_id() != context.correlation_id()
            || *run_id != context.run_id()
            || *task_id != context.task_id()
            || *outcome != TaskOutcome::Success
        {
            return Err(RuntimeHostError::fatal(
                "policy_run_receipt_identity_mismatch",
                "complete_scheduled_policy_run",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        self.validate_scheduled_policy_terminal(
            context,
            terminal,
            Some(receipt.request_id()),
            TaskOutcome::Success,
            None,
        )?;
        let execution_input = PolicyExecutionInput::Succeeded;
        let replayed = {
            let policy = lock(&self.policy, "replay_scheduled_policy_completion")?;
            policy
                .replay_execution(context.decision_id(), &execution_input)?
                .is_some()
                && !policy.dispatch_needs_completion(context.decision_id())?
        };
        let authoritative_outcome =
            self.read_scheduled_policy_outcome(context, terminal, receipt.request_id(), replayed)?;
        let execution = self.record_policy_dispatch_outcome_under_gate(
            context.decision_id(),
            &execution_input,
            Some(context),
            PolicyOutcomeCacheUpdate::Commit(
                authoritative_outcome
                    .as_ref()
                    .map(SchedulingOutcomeProjection::outcome),
            ),
        )?;
        let authoritative_outcome = authoritative_outcome
            .map(|projection| {
                let settlement_position = self
                    .ledger
                    .latest_sequence()
                    .map_err(|_| ledger_error("read_policy_recompute_position"))?;
                if settlement_position < projection.ledger_position() {
                    return Err(RuntimeHostError::fatal(
                        "policy_recompute_position_regressed",
                        "complete_scheduled_policy_run",
                        RuntimeErrorCode::RuntimeFatal,
                    ));
                }
                SchedulingOutcomeProjection::new(settlement_position, projection.outcome().clone())
                    .map_err(|_| {
                        RuntimeHostError::fatal(
                            "policy_recompute_projection_invalid",
                            "complete_scheduled_policy_run",
                            RuntimeErrorCode::RuntimeFatal,
                        )
                    })
            })
            .transpose()?;
        Ok((execution, authoritative_outcome))
    }

    fn read_scheduled_policy_outcome(
        &self,
        context: &PolicyRunContext,
        terminal: TerminalEvent,
        request_id: RequestId,
        completed_replay: bool,
    ) -> RuntimeHostResult<Option<SchedulingOutcomeProjection>> {
        let expected_keys = {
            let policy = lock(&self.policy, "read_policy_outcome_keys")?;
            if completed_replay {
                policy.referenced_outcome_keys_for_completed_run(context)?
            } else {
                policy.referenced_outcome_keys(context)?
            }
        };
        if expected_keys.is_empty() {
            return Ok(None);
        }
        let identity = SchedulingOutcomeIdentity::new(
            terminal.event_id,
            terminal.sequence,
            context.lease_token().instance_id(),
            context.task_id(),
            context.run_id(),
            request_id,
            context.correlation_id(),
            context.lease_token().lease_id(),
            context.decision_id(),
            context.catalog_task_id(),
            context.instance_alias(),
        )
        .map_err(|_| {
            RuntimeHostError::fatal(
                "policy_outcome_identity_invalid",
                "read_scheduled_policy_outcome",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        let terminal_events = self
            .ledger
            .query_page(
                EventQuery {
                    from_sequence: Some(terminal.sequence),
                    to_sequence: Some(terminal.sequence),
                    event_type: Some(EventType::TaskCompleted),
                    instance_id: Some(identity.instance_id()),
                    request_id: Some(identity.request_id()),
                    correlation_id: Some(identity.correlation_id()),
                    task_id: Some(identity.task_id()),
                    run_id: Some(identity.run_id()),
                    lease_id: Some(identity.lease_id()),
                    ..EventQuery::default()
                },
                terminal.sequence.saturating_sub(1),
                terminal.sequence,
                2,
            )
            .map_err(|_| ledger_error("read_policy_outcome_terminal"))?;
        let [terminal_event] = terminal_events.as_slice() else {
            return Err(RuntimeHostError::fatal(
                "policy_outcome_terminal_not_unique",
                "read_scheduled_policy_outcome",
                RuntimeErrorCode::RuntimeFatal,
            ));
        };
        let EventPayload::Task(TaskPayload::Semantic(payload)) = terminal_event.payload() else {
            return Err(RuntimeHostError::fatal(
                "policy_outcome_terminal_invalid",
                "read_scheduled_policy_outcome",
                RuntimeErrorCode::RuntimeFatal,
            ));
        };
        if matches!(
            payload.fact(),
            TaskSemanticFact::TerminalCommitted {
                scheduling_disposition: None,
                ..
            }
        ) {
            return Err(RuntimeHostError::fatal(
                "policy_run_terminal_disposition_missing",
                "read_scheduled_policy_outcome",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        validate_policy_run_admission_request(
            &self.ledger,
            context.lease_token().instance_id(),
            context.admission_request_id(),
            context.correlation_id(),
            context.task_id(),
            context.run_id(),
            context.decision_id(),
            context.catalog_task_id(),
            context.instance_alias(),
            terminal.sequence,
        )?;
        let ledger_position = self
            .ledger
            .latest_sequence()
            .map_err(|_| ledger_error("read_policy_outcome_position"))?;
        if ledger_position < terminal.sequence {
            return Err(RuntimeHostError::request(
                "outcome_projection_not_ready",
                "read_scheduled_policy_outcome",
                RuntimeErrorCode::RuntimeUnavailable,
            ));
        }
        let projection = self
            .ledger
            .project_scheduling_outcomes(identity.clone(), ledger_position)
            .map_err(|error| {
                if error.code() == "outcome_projection_not_ready" {
                    RuntimeHostError::request(
                        "outcome_projection_not_ready",
                        "read_scheduled_policy_outcome",
                        RuntimeErrorCode::RuntimeUnavailable,
                    )
                } else {
                    ledger_error("project_policy_outcome")
                }
            })?;
        if projection.ledger_position() < terminal.sequence
            || projection.outcome().identity() != &identity
            || !expected_keys.contains(projection.outcome().disposition().outcome_key())
        {
            return Err(RuntimeHostError::fatal(
                "policy_outcome_projection_mismatch",
                "read_scheduled_policy_outcome",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        Ok(Some(projection))
    }

    fn apply_policy_outcome_cache_update(
        &self,
        update: PolicyOutcomeCacheUpdate<'_>,
    ) -> RuntimeHostResult<()> {
        let mut outcomes = lock(
            &self.authoritative_policy_outcomes,
            "update_policy_scheduling_outcome",
        )?;
        match update {
            #[cfg(test)]
            PolicyOutcomeCacheUpdate::Retain => Ok(()),
            PolicyOutcomeCacheUpdate::Commit(Some(outcome)) => {
                insert_authoritative_policy_outcome(&mut outcomes, outcome.clone())
            }
            PolicyOutcomeCacheUpdate::Commit(None) => Ok(()),
            PolicyOutcomeCacheUpdate::Clear(context) => {
                outcomes.remove(&(
                    context.catalog_task_id().to_owned(),
                    context.instance_alias().to_owned(),
                ));
                Ok(())
            }
        }
    }

    pub(super) fn record_scheduled_policy_failure(
        &self,
        context: &PolicyRunContext,
        error: &RuntimeHostError,
    ) -> RuntimeHostResult<()> {
        // The task terminal or an earlier diagnostic may already carry this exact error.
        // Retain its recorded identity so retries of completion do not duplicate warnings.
        if error.projection().code == RuntimeErrorCode::LedgerFailure {
            return Err(error.clone());
        }
        let links = self.policy_run_event_links(context)?;
        if error.diagnostics().recorded_event().get().is_some() {
            return self.append_lifecycle_failure(
                RuntimeLifecycleFailureStage::OperationCleanup,
                RuntimeLifecycleFailure::Host(error),
                links,
                None,
            );
        }
        let failure = self.append_event_raw(
            if error.is_fatal() {
                EventSeverity::Fatal
            } else {
                EventSeverity::Warning
            },
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links.clone(),
            match error.resource_declaration() {
                Some(rejection) => {
                    RuntimePayloadDraft::resource_declaration_rejected(rejection.clone())
                }
                None => RuntimePayloadDraft::failed(
                    DiagnosticCode::RuntimeDiagnostic,
                    EffectDisposition::Indeterminate,
                    DiagnosticDetailDraft::new(
                        "policy_driver",
                        RuntimeLifecycleFailureStage::PolicyDriver.as_str(),
                        "runtime_host",
                        error.operation(),
                        error.code(),
                        Sensitivity::Internal,
                    ),
                    AuditInput::new(),
                ),
            },
        )?;
        self.record_required_failure(error, &failure, links)
    }

    pub(super) fn complete_scheduled_policy_failure(
        &self,
        context: &PolicyRunContext,
        failure: &RequestFailure,
    ) -> RuntimeHostResult<PolicyExecutionEventData> {
        let _gate = lock(
            &self.policy_outcome_gate,
            "complete_scheduled_policy_failure",
        )?;
        if !matches!(
            failure.state,
            RuntimeReceiptState::Denied
                | RuntimeReceiptState::Failed
                | RuntimeReceiptState::Cancelled
        ) {
            return Err(RuntimeHostError::fatal(
                "policy_run_failure_state_invalid",
                "complete_scheduled_policy_failure",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        self.ensure_scheduled_policy_lease_released(context)?;
        let input = self.scheduled_policy_failure_input(context, failure)?;
        self.record_policy_dispatch_outcome_under_gate(
            context.decision_id(),
            &input,
            Some(context),
            PolicyOutcomeCacheUpdate::Clear(context),
        )
    }

    pub(super) fn ensure_scheduled_policy_lease_released(
        &self,
        context: &PolicyRunContext,
    ) -> RuntimeHostResult<()> {
        let release_query = || {
            self.ledger
                .query(EventQuery {
                    event_type: Some(EventType::LeaseReleased),
                    correlation_id: Some(context.correlation_id()),
                    task_id: Some(context.task_id()),
                    run_id: Some(context.run_id()),
                    lease_id: Some(context.lease_token().lease_id()),
                    ..EventQuery::default()
                })
                .map_err(|_| ledger_error("validate_policy_run_release"))
        };
        let existing = release_query()?;
        if existing.is_empty() {
            let request = context.request().validate().map_err(|_| {
                RuntimeHostError::fatal(
                    "policy_run_request_invalid",
                    "release_failed_policy_run",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
            let connection_id = ConnectionId::new(POLICY_CONNECTION_VALUE)
                .map_err(|error| RuntimeHostError::scheduler("build_policy_connection", &error))?;
            self.cleanup_scheduled_failure_with_run_links(
                &request,
                context.lease_token(),
                connection_id,
                RuntimeRunLinks::new(context.issued_task_id(), context.issued_run_id()),
            )?;
        }
        if release_query()?.len() != 1 {
            return Err(RuntimeHostError::fatal(
                "policy_run_release_missing",
                "complete_scheduled_policy_failure",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        Ok(())
    }

    fn scheduled_policy_failure_input(
        &self,
        context: &PolicyRunContext,
        failure: &RequestFailure,
    ) -> RuntimeHostResult<PolicyExecutionInput> {
        let task_terminals = self
            .ledger
            .query(EventQuery {
                correlation_id: Some(context.correlation_id()),
                task_id: Some(context.task_id()),
                run_id: Some(context.run_id()),
                lease_id: Some(context.lease_token().lease_id()),
                ..EventQuery::default()
            })
            .map_err(|_| ledger_error("validate_policy_run_failure"))?;
        let task_terminals = task_terminals
            .iter()
            .filter(|event| {
                matches!(
                    event.event_type(),
                    EventType::TaskCompleted | EventType::TaskFailed | EventType::TaskCancelled
                )
            })
            .collect::<Vec<_>>();
        let task_failure = match task_terminals.as_slice() {
            [] => {
                return Ok(PolicyExecutionInput::Failed {
                    error_code: failure.error.code().to_owned(),
                    class: if failure.error.is_fatal() {
                        PolicyFailureClass::Severe
                    } else {
                        PolicyFailureClass::Recoverable
                    },
                });
            }
            [task_failure] if task_failure.event_type() == EventType::TaskFailed => *task_failure,
            _ => {
                return Err(RuntimeHostError::fatal(
                    "policy_run_failure_terminal_conflict",
                    "complete_scheduled_policy_failure",
                    RuntimeErrorCode::RuntimeFatal,
                ));
            }
        };
        let class = match task_failure.severity() {
            EventSeverity::Warning => PolicyFailureClass::Recoverable,
            EventSeverity::Fatal => PolicyFailureClass::Severe,
            EventSeverity::Debug | EventSeverity::Info | EventSeverity::Error => {
                return Err(RuntimeHostError::fatal(
                    "policy_run_failure_severity_ambiguous",
                    "complete_scheduled_policy_failure",
                    RuntimeErrorCode::RuntimeFatal,
                ));
            }
        };
        let EventPayload::Task(TaskPayload::Semantic(payload)) = task_failure.payload() else {
            return Err(RuntimeHostError::fatal(
                "policy_run_failure_terminal_invalid",
                "complete_scheduled_policy_failure",
                RuntimeErrorCode::RuntimeFatal,
            ));
        };
        let TaskSemanticFact::TerminalCommitted {
            outcome: TaskOutcome::Failure,
            failure_code: Some(code),
            ..
        } = payload.fact()
        else {
            return Err(RuntimeHostError::fatal(
                "policy_run_failure_terminal_invalid",
                "complete_scheduled_policy_failure",
                RuntimeErrorCode::RuntimeFatal,
            ));
        };
        if failure.terminal.is_none_or(|terminal| {
            terminal.sequence != task_failure.sequence()
                || terminal.event_id != *task_failure.event_id()
        }) {
            return Err(RuntimeHostError::fatal(
                "policy_run_failure_terminal_mismatch",
                "complete_scheduled_policy_failure",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        Ok(PolicyExecutionInput::Failed {
            error_code: code.clone(),
            class,
        })
    }

    fn validate_scheduled_policy_terminal(
        &self,
        context: &PolicyRunContext,
        terminal: TerminalEvent,
        request_id: Option<RequestId>,
        expected_outcome: TaskOutcome,
        expected_failure_code: Option<&str>,
    ) -> RuntimeHostResult<()> {
        let through_sequence = self
            .ledger
            .latest_sequence()
            .map_err(|_| ledger_error("validate_policy_run_terminal"))?;
        let mut terminal_events = Vec::new();
        for event_type in [
            EventType::TaskCompleted,
            EventType::TaskFailed,
            EventType::TaskCancelled,
        ] {
            let remaining = 2_usize.saturating_sub(terminal_events.len());
            if remaining == 0 {
                break;
            }
            terminal_events.extend(
                self.ledger
                    .query_page(
                        EventQuery {
                            event_type: Some(event_type),
                            instance_id: Some(context.lease_token().instance_id()),
                            correlation_id: Some(context.correlation_id()),
                            task_id: Some(context.task_id()),
                            run_id: Some(context.run_id()),
                            lease_id: Some(context.lease_token().lease_id()),
                            ..EventQuery::default()
                        },
                        0,
                        through_sequence,
                        remaining,
                    )
                    .map_err(|_| ledger_error("validate_policy_run_terminal"))?,
            );
        }
        terminal_events.sort_by_key(PersistedEvent::sequence);
        let [terminal_event] = terminal_events.as_slice() else {
            return Err(RuntimeHostError::fatal(
                "policy_run_receipt_terminal_mismatch",
                "validate_scheduled_policy_terminal",
                RuntimeErrorCode::RuntimeFatal,
            ));
        };
        let terminal_matches = matches!(
            terminal_event.payload(),
            EventPayload::Task(TaskPayload::Semantic(payload))
                if matches!(
                    payload.fact(),
                    TaskSemanticFact::TerminalCommitted {
                        outcome,
                        failure_code,
                        ..
                    } if *outcome == expected_outcome
                        && failure_code.as_deref() == expected_failure_code
                )
        );
        if !terminal_matches
            || terminal_event.sequence() != terminal.sequence
            || terminal_event.event_id() != &terminal.event_id
            || terminal_event.links().request_id() != request_id.as_ref()
            || terminal_event.links().correlation_id() != Some(&context.correlation_id())
        {
            return Err(RuntimeHostError::fatal(
                "policy_run_receipt_terminal_mismatch",
                "validate_scheduled_policy_terminal",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        let release_events = self
            .ledger
            .query(EventQuery {
                event_type: Some(EventType::LeaseReleased),
                run_id: Some(context.run_id()),
                task_id: Some(context.task_id()),
                lease_id: Some(context.lease_token().lease_id()),
                ..EventQuery::default()
            })
            .map_err(|_| ledger_error("validate_policy_run_release"))?;
        if release_events.len() != 1 {
            return Err(RuntimeHostError::fatal(
                "policy_run_release_missing",
                "validate_scheduled_policy_terminal",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn record_policy_dispatch_outcome(
        &self,
        decision_id: &str,
        input: &PolicyExecutionInput,
        context: Option<&PolicyRunContext>,
    ) -> RuntimeHostResult<PolicyExecutionEventData> {
        self.record_policy_dispatch_outcome_with_cache_update(
            decision_id,
            input,
            context,
            PolicyOutcomeCacheUpdate::Retain,
        )
    }

    #[cfg(test)]
    pub(super) fn record_policy_dispatch_outcome_with_cache_update(
        &self,
        decision_id: &str,
        input: &PolicyExecutionInput,
        context: Option<&PolicyRunContext>,
        cache_update: PolicyOutcomeCacheUpdate<'_>,
    ) -> RuntimeHostResult<PolicyExecutionEventData> {
        let _gate = lock(&self.policy_outcome_gate, "record_policy_dispatch_outcome")?;
        self.record_policy_dispatch_outcome_under_gate(decision_id, input, context, cache_update)
    }

    fn record_policy_dispatch_outcome_under_gate(
        &self,
        decision_id: &str,
        input: &PolicyExecutionInput,
        context: Option<&PolicyRunContext>,
        cache_update: PolicyOutcomeCacheUpdate<'_>,
    ) -> RuntimeHostResult<PolicyExecutionEventData> {
        let result: RuntimeHostResult<PolicyExecutionEventData> = (|| {
            if let Some(context) = context
                && context.decision_id() != decision_id
            {
                return Err(RuntimeHostError::fatal(
                    "policy_run_decision_mismatch",
                    "record_policy_dispatch_outcome",
                    RuntimeErrorCode::RuntimeFatal,
                ));
            }
            let replay = lock(&self.policy, "replay_policy_dispatch_outcome")?
                .replay_execution(decision_id, input)?;
            if let Some(existing) = replay {
                let mut policy = lock(&self.policy, "finish_replayed_policy_outcome")?;
                self.finish_policy_dispatch_outcome(&mut policy, &existing, context, cache_update)?;
                let settlement = policy
                    .latest_settlement(&existing.task_id, &existing.instance_id)?
                    .ok_or_else(policy_settlement_missing)?;
                drop(policy);
                self.record_policy_settlement_facts(&settlement)?;
                return Ok(existing);
            }
            if let Some(context) = context {
                lock(&self.policy, "validate_policy_run_context")?.validate_run_context(context)?;
            }
            let (instance_id, admitted_at_unix_ms) = {
                let policy = lock(&self.policy, "read_policy_dispatch_instance")?;
                (
                    policy.execution_instance_id(decision_id)?.to_owned(),
                    policy.admitted_at(decision_id)?,
                )
            };
            let sample = self.runtime_clock_sample()?;
            let clock = lock(&self.policy_dispatch_clocks, "read_policy_dispatch_start")?
                .get(decision_id)
                .copied()
                .ok_or_else(|| {
                    RuntimeHostError::fatal(
                        "policy_dispatch_clock_missing",
                        "record_policy_dispatch_outcome",
                        RuntimeErrorCode::RuntimeFatal,
                    )
                })?;
            if clock.admitted_at_unix_ms != admitted_at_unix_ms {
                return Err(RuntimeHostError::fatal(
                    "policy_dispatch_clock_identity_conflict",
                    "record_policy_dispatch_outcome",
                    RuntimeErrorCode::RuntimeFatal,
                ));
            }
            let runtime_ms = match clock.started_at_monotonic_ms {
                Some(started_at_monotonic_ms) => {
                    sample.monotonic_ms.checked_sub(started_at_monotonic_ms)
                }
                None => sample.unix_ms.checked_sub(admitted_at_unix_ms),
            }
            .ok_or_else(|| {
                RuntimeHostError::fatal(
                    "policy_dispatch_clock_regressed",
                    "record_policy_dispatch_outcome",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
            let observed_at_unix_ms =
                admitted_at_unix_ms.checked_add(runtime_ms).ok_or_else(|| {
                    RuntimeHostError::fatal(
                        "policy_execution_time_overflow",
                        "record_policy_dispatch_outcome",
                        RuntimeErrorCode::RuntimeFatal,
                    )
                })?;
            let perf_context = self.performance_context(&instance_id, observed_at_unix_ms)?;
            let mut policy = lock(&self.policy, "record_policy_dispatch_outcome")?;
            let data = match policy.prepare_execution(
                decision_id,
                observed_at_unix_ms,
                runtime_ms,
                input,
                &perf_context,
            )? {
                PolicyExecutionPreparation::New(data) => {
                    let links = match context {
                        Some(context) => self.policy_run_event_links(context)?,
                        None => self.events.system_links()?,
                    };
                    #[cfg(test)]
                    self.consume_scheduled_policy_checkpoint_for_test(context)?;
                    #[cfg(test)]
                    fail_policy_execution_append_for_test()?;
                    self.append_event_raw(
                        policy_execution_severity(&data),
                        EventSource::Scheduler,
                        OriginModule::Policy,
                        EventActor::Scheduler,
                        links,
                        PolicyPayloadDraft::execution_recorded(data.clone(), AuditInput::new()),
                    )?;
                    #[cfg(test)]
                    policy_crash_test_barrier("after_policy_execution");
                    policy.commit_execution(&data)?;
                    data
                }
                PolicyExecutionPreparation::Replay(data) => data,
            };
            self.finish_policy_dispatch_outcome(&mut policy, &data, context, cache_update)?;
            // The settled run's four settlement facts (Workflow #308 slice 5b).
            let settlement = policy
                .latest_settlement(&data.task_id, &data.instance_id)?
                .ok_or_else(policy_settlement_missing)?;
            drop(policy);
            self.record_policy_settlement_facts(&settlement)?;
            Ok(data)
        })();
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.fatal.mark(error.clone())?;
        }
        result
    }

    // Caller holds policy_outcome_gate across execution, completion and cache publication.
    fn finish_policy_dispatch_outcome(
        &self,
        policy: &mut PolicyHost,
        data: &PolicyExecutionEventData,
        context: Option<&PolicyRunContext>,
        cache_update: PolicyOutcomeCacheUpdate<'_>,
    ) -> RuntimeHostResult<()> {
        let decision_id = &data.decision_id;
        if policy.dispatch_needs_completion(decision_id)? {
            let (dispatch, admission) = policy.completion_data(decision_id)?;
            let completes_decision = |event: &PersistedEvent| {
                matches!(
                    event.payload(),
                    EventPayload::Policy(PolicyPayload::DispatchCompleted(payload))
                        if payload.decision_id() == decision_id.as_str()
                )
            };
            // A scheduled run's completion carries that run's links, whether this host or the
            // ledger's settlement recovery appended it: read them through the link index
            // (Workflow #317 item C). Without a run context every completion is read.
            let completions = match context {
                Some(context) => linked_policy_run_events(
                    &self.ledger,
                    EventQuery {
                        event_type: Some(EventType::PolicyDispatchCompleted),
                        correlation_id: Some(context.correlation_id()),
                        task_id: Some(context.task_id()),
                        run_id: Some(context.run_id()),
                        ..EventQuery::default()
                    },
                    u64::MAX,
                    2,
                    "read_policy_dispatch_completion",
                    completes_decision,
                )?,
                None => self
                    .ledger
                    .query(EventQuery {
                        event_type: Some(EventType::PolicyDispatchCompleted),
                        ..EventQuery::default()
                    })
                    .map_err(|_| ledger_error("read_policy_dispatch_completion"))?
                    .into_iter()
                    .filter(|event| completes_decision(event))
                    .collect::<Vec<_>>(),
            };
            let completion = match completions.as_slice() {
                [completion] => completion.clone(),
                [] => self.append_event_raw(
                    EventSeverity::Info,
                    EventSource::Scheduler,
                    OriginModule::Policy,
                    EventActor::Scheduler,
                    match context {
                        Some(context) => self.policy_run_event_links(context)?,
                        None => self.events.system_links()?,
                    },
                    PolicyPayloadDraft::dispatch_completed(dispatch, admission, AuditInput::new()),
                )?,
                _ => {
                    return Err(policy_admission_fatal(
                        "policy_dispatch_completion_not_unique",
                        "complete_policy_dispatch_outcome",
                    ));
                }
            };
            #[cfg(test)]
            policy_crash_test_barrier("after_policy_completion");
            policy.complete_dispatch(decision_id, &completion)?;
        }
        #[cfg(test)]
        self.wait_policy_outcome_transition_test_hook()?;
        self.apply_policy_outcome_cache_update(cache_update)?;
        lock(&self.policy_dispatch_clocks, "clear_policy_dispatch_start")?.remove(decision_id);
        Ok(())
    }

    fn policy_run_event_links(
        &self,
        context: &PolicyRunContext,
    ) -> RuntimeHostResult<EventLinksDraft> {
        let request = context.request().validate().map_err(|_| {
            RuntimeHostError::fatal(
                "policy_run_request_invalid",
                "build_policy_run_event_links",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        let action_id = self.events.action_id()?;
        Ok(
            RuntimeRunLinks::new(context.issued_task_id(), context.issued_run_id()).apply(
                self.events.request_links(
                    &request,
                    Some(context.lease_token().instance_id()),
                    Some(context.lease_token().lease_id()),
                    Some(action_id),
                ),
            ),
        )
    }

    #[cfg(test)]
    fn wait_policy_outcome_transition_test_hook(&self) -> RuntimeHostResult<()> {
        let hook = lock(
            &self.policy_outcome_transition_test_hook,
            "read_policy_outcome_transition_test_hook",
        )?
        .take();
        if let Some(hook) = hook {
            hook.completion_committed.wait();
            hook.resume.wait();
        }
        Ok(())
    }
}

pub(super) fn reconcile_policy_dispatches(
    policy: &mut PolicyHost,
    ledger: &GlobalLedger,
    events: &RuntimeEvents,
) -> RuntimeHostResult<()> {
    reconcile_scheduled_policy_outcomes(policy, ledger)?;
    for decision_id in policy.pending_dispatch_completions() {
        let execution = policy.execution_data(&decision_id)?;
        let completion = ledger
            .reconcile_scheduled_policy_settlement(execution)
            .map_err(|_| ledger_error("reconcile_policy_dispatches"))?;
        policy.complete_dispatch(&decision_id, &completion)?;
    }
    policy.refresh_dispatches(ledger)?;
    let pending = policy.pending_dispatches();
    if pending.is_empty() {
        return Ok(());
    }
    // Every read below is pinned to one position and goes through an index (Workflow #317
    // rf2), never the whole ledger; the original predicates still select the events.
    let through = ledger
        .latest_sequence()
        .map_err(|_| ledger_error("reconcile_policy_dispatches"))?;
    for dispatch in pending {
        let intent = policy_dispatch_intent(
            ledger,
            &dispatch.data.decision_id,
            through,
            "reconcile_policy_dispatches",
        )?;
        let lease_granted = !linked_policy_run_events(
            ledger,
            EventQuery {
                from_sequence: Some(intent.sequence().saturating_add(1)),
                to_sequence: Some(through),
                event_type: Some(EventType::LeaseGranted),
                instance_id: intent.links().instance_id().copied(),
                request_id: intent.links().request_id().copied(),
                correlation_id: intent.links().correlation_id().copied(),
                ..EventQuery::default()
            },
            through,
            1,
            "reconcile_policy_dispatches",
            |event| {
                event.sequence() > intent.sequence()
                    && event.event_type() == EventType::LeaseGranted
                    && event.links().request_id() == intent.links().request_id()
                    && event.links().correlation_id() == intent.links().correlation_id()
                    && event.links().instance_id() == intent.links().instance_id()
            },
        )?
        .is_empty();
        let effect = if lease_granted {
            EffectDisposition::Indeterminate
        } else {
            EffectDisposition::NotPerformed
        };
        let draft = events.draft(
            EventSeverity::Error,
            EventSource::Scheduler,
            OriginModule::Policy,
            EventActor::Scheduler,
            events.system_links()?,
            PolicyPayloadDraft::dispatch_rejected(dispatch.data, effect, AuditInput::new()),
        )?;
        let draft = events.sanitize(draft)?;
        ledger
            .append(draft)
            .map_err(|_| ledger_error("reconcile_policy_dispatches"))?;
    }
    policy.refresh_dispatches(ledger)
}

fn reconcile_scheduled_policy_outcomes(
    policy: &mut PolicyHost,
    ledger: &GlobalLedger,
) -> RuntimeHostResult<()> {
    let pending = policy.pending_dispatch_outcomes();
    reconcile_scheduled_policy_outcomes_for(policy, ledger, pending)
}

fn reconcile_scheduled_policy_outcomes_for(
    policy: &mut PolicyHost,
    ledger: &GlobalLedger,
    pending: Vec<String>,
) -> RuntimeHostResult<()> {
    if pending.is_empty() {
        return Ok(());
    }
    // Every read below is pinned to one position and goes through an index (Workflow #317
    // rf2), never the whole ledger; the original predicates still select the events.
    let through = ledger
        .latest_sequence()
        .map_err(|_| ledger_error("reconcile_policy_outcomes"))?;
    for decision_id in pending {
        let intent =
            &policy_dispatch_intent(ledger, &decision_id, through, "reconcile_policy_outcomes")?;
        let (
            Some(instance_id),
            Some(request_id),
            Some(correlation_id),
            Some(task_id),
            Some(run_id),
        ) = (
            intent.links().instance_id(),
            intent.links().request_id(),
            intent.links().correlation_id(),
            intent.links().task_id(),
            intent.links().run_id(),
        )
        else {
            return Err(policy_admission_fatal(
                "policy_run_identity_missing",
                "reconcile_policy_outcomes",
            ));
        };
        let lease_grants = linked_policy_run_events(
            ledger,
            EventQuery {
                to_sequence: Some(through),
                event_type: Some(EventType::LeaseGranted),
                instance_id: Some(*instance_id),
                request_id: Some(*request_id),
                correlation_id: Some(*correlation_id),
                task_id: Some(*task_id),
                run_id: Some(*run_id),
                ..EventQuery::default()
            },
            through,
            2,
            "reconcile_policy_outcomes",
            |event| {
                event.event_type() == EventType::LeaseGranted
                    && event.links().instance_id() == Some(instance_id)
                    && event.links().request_id() == Some(request_id)
                    && event.links().correlation_id() == Some(correlation_id)
                    && event.links().task_id() == Some(task_id)
                    && event.links().run_id() == Some(run_id)
            },
        )?;
        let [lease_granted] = lease_grants.as_slice() else {
            return Err(policy_admission_fatal(
                "policy_run_lease_fact_not_unique",
                "reconcile_policy_outcomes",
            ));
        };
        let Some(lease_id) = lease_granted.links().lease_id() else {
            return Err(policy_admission_fatal(
                "policy_run_identity_missing",
                "reconcile_policy_outcomes",
            ));
        };
        let mut terminals = Vec::new();
        for terminal_type in [
            EventType::TaskCompleted,
            EventType::TaskFailed,
            EventType::TaskCancelled,
        ] {
            if terminals.len() > 1 {
                break;
            }
            terminals.extend(linked_policy_run_events(
                ledger,
                EventQuery {
                    to_sequence: Some(through),
                    event_type: Some(terminal_type),
                    correlation_id: Some(*correlation_id),
                    task_id: Some(*task_id),
                    run_id: Some(*run_id),
                    lease_id: Some(*lease_id),
                    ..EventQuery::default()
                },
                through,
                2 - terminals.len(),
                "reconcile_policy_outcomes",
                |event| {
                    matches!(
                        event.event_type(),
                        EventType::TaskCompleted | EventType::TaskFailed | EventType::TaskCancelled
                    ) && event.links().correlation_id() == Some(correlation_id)
                        && event.links().task_id() == Some(task_id)
                        && event.links().run_id() == Some(run_id)
                        && event.links().lease_id() == Some(lease_id)
                },
            )?);
        }
        let (observed_at_unix_ms, input, runtime_ms) = match terminals.as_slice() {
            [terminal] if terminal.event_type() == EventType::TaskCompleted => {
                let runtime_ms = recovered_scheduled_task_runtime_ms(ledger, through, terminal)?;
                (
                    terminal.timestamp_unix_ms(),
                    PolicyExecutionInput::Succeeded,
                    runtime_ms,
                )
            }
            [terminal] if terminal.event_type() == EventType::TaskFailed => {
                let class = match terminal.severity() {
                    EventSeverity::Warning => PolicyFailureClass::Recoverable,
                    EventSeverity::Fatal => PolicyFailureClass::Severe,
                    EventSeverity::Debug | EventSeverity::Info | EventSeverity::Error => {
                        return Err(policy_admission_fatal(
                            "policy_run_failure_severity_ambiguous",
                            "reconcile_policy_outcomes",
                        ));
                    }
                };
                let EventPayload::Task(TaskPayload::Semantic(payload)) = terminal.payload() else {
                    return Err(policy_admission_fatal(
                        "policy_run_terminal_invalid",
                        "reconcile_policy_outcomes",
                    ));
                };
                let TaskSemanticFact::TerminalCommitted {
                    outcome: TaskOutcome::Failure,
                    failure_code: Some(error_code),
                    ..
                } = payload.fact()
                else {
                    return Err(policy_admission_fatal(
                        "policy_run_terminal_invalid",
                        "reconcile_policy_outcomes",
                    ));
                };
                let runtime_ms = recovered_scheduled_task_runtime_ms(ledger, through, terminal)?;
                (
                    terminal.timestamp_unix_ms(),
                    PolicyExecutionInput::Failed {
                        error_code: error_code.clone(),
                        class,
                    },
                    runtime_ms,
                )
            }
            [] => {
                let releases = linked_policy_run_events(
                    ledger,
                    EventQuery {
                        to_sequence: Some(through),
                        event_type: Some(EventType::LeaseReleased),
                        instance_id: Some(*instance_id),
                        request_id: Some(*request_id),
                        correlation_id: Some(*correlation_id),
                        task_id: Some(*task_id),
                        run_id: Some(*run_id),
                        lease_id: Some(*lease_id),
                        ..EventQuery::default()
                    },
                    through,
                    2,
                    "reconcile_policy_outcomes",
                    |event| scheduled_admission_release_matches(event, intent, lease_id),
                )?;
                let release = match releases.as_slice() {
                    [] => continue,
                    [release] => release,
                    _ => {
                        return Err(policy_admission_fatal(
                            "policy_run_release_fact_not_unique",
                            "reconcile_policy_outcomes",
                        ));
                    }
                };
                (
                    release.timestamp_unix_ms(),
                    PolicyExecutionInput::Failed {
                        error_code: "policy_settlement_interrupted".to_owned(),
                        class: PolicyFailureClass::Severe,
                    },
                    0,
                )
            }
            _ => {
                return Err(policy_admission_fatal(
                    "policy_run_terminal_conflict",
                    "reconcile_policy_outcomes",
                ));
            }
        };
        let data = match policy.prepare_execution(
            &decision_id,
            observed_at_unix_ms,
            runtime_ms,
            &input,
            &PerformanceContext::unavailable(observed_at_unix_ms),
        )? {
            PolicyExecutionPreparation::New(data) => data,
            PolicyExecutionPreparation::Replay(_) => {
                return Err(policy_admission_fatal(
                    "policy_execution_recovery_conflict",
                    "reconcile_policy_outcomes",
                ));
            }
        };
        if !policy_recovery_outcome_matches(&data.outcome, &input) {
            return Err(policy_admission_fatal(
                "policy_execution_recovery_outcome_conflict",
                "reconcile_policy_outcomes",
            ));
        }
        let completion = ledger
            .reconcile_scheduled_policy_settlement(data.clone())
            .map_err(|_| ledger_error("reconcile_policy_outcomes"))?;
        policy.commit_execution(&data)?;
        policy.complete_dispatch(&decision_id, &completion)?;
    }
    Ok(())
}

fn scheduled_admission_release_matches(
    event: &PersistedEvent,
    intent: &PersistedEvent,
    lease_id: &LeaseId,
) -> bool {
    event.event_type() == EventType::LeaseReleased
        && event.links().instance_id() == intent.links().instance_id()
        && event.links().request_id() == intent.links().request_id()
        && event.links().correlation_id() == intent.links().correlation_id()
        && event.links().task_id() == intent.links().task_id()
        && event.links().run_id() == intent.links().run_id()
        && event.links().lease_id() == Some(lease_id)
}

/// The events of one link-indexed query through `through` that `keep` selects, read two per
/// page and stopping at `cap` (Workflow #317 items B and C). The query only narrows the rows
/// read; `keep` is the original predicate over the whole ledger.
fn linked_policy_run_events(
    ledger: &GlobalLedger,
    query: EventQuery,
    through: u64,
    cap: usize,
    operation: &'static str,
    mut keep: impl FnMut(&PersistedEvent) -> bool,
) -> RuntimeHostResult<Vec<PersistedEvent>> {
    const LINKED_PAGE_EVENTS: usize = 2;
    let mut selected = Vec::new();
    let mut after = 0;
    loop {
        let page = ledger
            .query_page(query.clone(), after, through, LINKED_PAGE_EVENTS)
            .map_err(|_| ledger_error(operation))?;
        let exhausted = page.len() < LINKED_PAGE_EVENTS;
        if let Some(last) = page.last() {
            after = last.sequence();
        }
        for event in page {
            if keep(&event) {
                selected.push(event);
                if selected.len() >= cap {
                    return Ok(selected);
                }
            }
        }
        if exhausted {
            return Ok(selected);
        }
    }
}

/// The one `PolicyDispatchIntent` of `decision_id` through `through`, read with two-event
/// pages of the intent type index (Workflow #317 rf2). No link names a decision, so the page
/// query is the event type and the decision is the filter; none or more than one is
/// `policy_dispatch_intent_missing`.
fn policy_dispatch_intent(
    ledger: &GlobalLedger,
    decision_id: &str,
    through: u64,
    operation: &'static str,
) -> RuntimeHostResult<PersistedEvent> {
    let mut intents = linked_policy_run_events(
        ledger,
        EventQuery {
            to_sequence: Some(through),
            event_type: Some(EventType::PolicyDispatchIntent),
            ..EventQuery::default()
        },
        through,
        2,
        operation,
        |event| {
            matches!(
                event.payload(),
                EventPayload::Policy(PolicyPayload::DispatchIntent(payload))
                    if payload.decision_id() == decision_id
            )
        },
    )?;
    match (intents.pop(), intents.is_empty()) {
        (Some(intent), true) => Ok(intent),
        _ => Err(policy_admission_fatal(
            "policy_dispatch_intent_missing",
            operation,
        )),
    }
}

/// Whether any effect of this run was started, read through its task and run links.
fn policy_run_effect_started(
    ledger: &GlobalLedger,
    links: &actingcommand_contract::EventLinks,
    through: u64,
) -> RuntimeHostResult<bool> {
    for effect_type in [
        EventType::TaskEffectIntent,
        EventType::TaskEffectCompleted,
        EventType::InputIntent,
        EventType::InputCommitted,
        EventType::InputFailed,
    ] {
        let started = linked_policy_run_events(
            ledger,
            EventQuery {
                to_sequence: Some(through),
                event_type: Some(effect_type),
                task_id: links.task_id().copied(),
                run_id: links.run_id().copied(),
                ..EventQuery::default()
            },
            through,
            1,
            "select_policy_settlements",
            |event| {
                event.links().task_id() == links.task_id()
                    && event.links().run_id() == links.run_id()
                    && matches!(
                        event.event_type(),
                        EventType::TaskEffectIntent
                            | EventType::TaskEffectCompleted
                            | EventType::InputIntent
                            | EventType::InputCommitted
                            | EventType::InputFailed
                    )
            },
        )?;
        if !started.is_empty() {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn recover_authoritative_policy_outcomes(
    policy: &PolicyHost,
    ledger: &GlobalLedger,
) -> RuntimeHostResult<BTreeMap<(String, String), AuthoritativeSchedulingOutcome>> {
    let mut completed = policy.completed_policy_runs(MAX_AUTHORITATIVE_POLICY_OUTCOMES)?;
    completed.sort_by_key(|run| run.completion_sequence);
    let ledger_position = ledger
        .latest_sequence()
        .map_err(|_| ledger_error("recover_policy_outcome_position"))?;
    let mut outcomes = BTreeMap::new();
    for run in completed {
        let key = (run.catalog_task_id.clone(), run.instance_alias.clone());
        if matches!(run.execution_outcome, PolicyExecutionOutcome::Failed { .. }) {
            outcomes.remove(&key);
            continue;
        }
        let terminal = recover_scheduled_terminal(ledger, &run)?.ok_or_else(|| {
            policy_admission_fatal(
                "policy_run_terminal_missing",
                "recover_policy_scheduling_outcomes",
            )
        })?;
        let EventPayload::Task(TaskPayload::Semantic(payload)) = terminal.payload() else {
            return Err(policy_admission_fatal(
                "policy_run_terminal_invalid",
                "recover_policy_scheduling_outcomes",
            ));
        };
        let TaskSemanticFact::TerminalCommitted {
            outcome: TaskOutcome::Success,
            failure_code: None,
            scheduling_disposition: Some(_),
            ..
        } = payload.fact()
        else {
            return Err(policy_admission_fatal(
                "policy_run_terminal_disposition_missing",
                "recover_policy_scheduling_outcomes",
            ));
        };
        validate_completed_run_admission_request(ledger, &run, terminal.sequence())?;
        let identity = SchedulingOutcomeIdentity::new(
            *terminal.event_id(),
            terminal.sequence(),
            run.instance_id,
            run.task_id,
            run.run_id,
            *terminal.links().request_id().ok_or_else(|| {
                policy_admission_fatal(
                    "policy_run_identity_missing",
                    "recover_policy_scheduling_outcomes",
                )
            })?,
            run.correlation_id,
            run.lease_id,
            run.decision_id,
            run.catalog_task_id,
            run.instance_alias,
        )
        .map_err(|_| {
            policy_admission_fatal(
                "policy_outcome_identity_invalid",
                "recover_policy_scheduling_outcomes",
            )
        })?;
        let projected = ledger
            .project_scheduling_outcomes(identity, ledger_position)
            .map_err(|_| ledger_error("recover_policy_scheduling_outcomes"))?;
        insert_authoritative_policy_outcome(&mut outcomes, projected.outcome().clone())?;
    }
    Ok(outcomes)
}

pub(super) fn validate_completed_run_admission_request(
    ledger: &GlobalLedger,
    run: &CompletedPolicyRunIdentity,
    through_sequence: u64,
) -> RuntimeHostResult<()> {
    validate_policy_run_admission_request(
        ledger,
        run.instance_id,
        run.admission_request_id,
        run.correlation_id,
        run.task_id,
        run.run_id,
        &run.decision_id,
        &run.catalog_task_id,
        &run.instance_alias,
        through_sequence,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn validate_policy_run_admission_request(
    ledger: &GlobalLedger,
    instance_id: InstanceId,
    admission_request_id: RequestId,
    correlation_id: CorrelationId,
    task_id: TaskId,
    run_id: RunId,
    decision_id: &str,
    catalog_task_id: &str,
    instance_alias: &str,
    through_sequence: u64,
) -> RuntimeHostResult<()> {
    let admissions = ledger
        .query_page(
            EventQuery {
                to_sequence: Some(through_sequence),
                event_type: Some(EventType::PolicyDispatchAdmitted),
                instance_id: Some(instance_id),
                correlation_id: Some(correlation_id),
                task_id: Some(task_id),
                run_id: Some(run_id),
                ..EventQuery::default()
            },
            0,
            through_sequence,
            2,
        )
        .map_err(|_| ledger_error("validate_policy_scheduling_admission"))?;
    let [admission] = admissions.as_slice() else {
        return Err(policy_admission_fatal(
            "policy_run_admission_conflict",
            "recover_policy_scheduling_outcomes",
        ));
    };
    let EventPayload::Policy(PolicyPayload::DispatchAdmitted(payload)) = admission.payload() else {
        return Err(policy_admission_fatal(
            "policy_run_admission_invalid",
            "recover_policy_scheduling_outcomes",
        ));
    };
    if admission.links().request_id() != Some(&admission_request_id)
        || payload.decision_id() != decision_id
        || payload.task_id() != catalog_task_id
        || payload.instance_id() != instance_alias
    {
        return Err(policy_admission_fatal(
            "policy_run_admission_conflict",
            "recover_policy_scheduling_outcomes",
        ));
    }
    Ok(())
}

pub(super) fn completed_run_matches_outcome(
    completed: &CompletedPolicyRunIdentity,
    outcome: &AuthoritativeSchedulingOutcome,
) -> bool {
    let identity = outcome.identity();
    completed.decision_id == identity.decision_id()
        && completed.catalog_task_id == identity.catalog_task_id()
        && completed.instance_alias == identity.instance_alias()
        && completed.instance_id == identity.instance_id()
        && completed.correlation_id == identity.correlation_id()
        && completed.task_id == identity.task_id()
        && completed.run_id == identity.run_id()
        && completed.lease_id == identity.lease_id()
        && identity.terminal_sequence() < completed.completion_sequence
}

fn recover_scheduled_terminal(
    ledger: &GlobalLedger,
    run: &CompletedPolicyRunIdentity,
) -> RuntimeHostResult<Option<PersistedEvent>> {
    let through_sequence = run.completion_sequence.saturating_sub(1);
    let mut terminals = Vec::new();
    for event_type in [
        EventType::TaskCompleted,
        EventType::TaskFailed,
        EventType::TaskCancelled,
    ] {
        terminals.extend(
            ledger
                .query_page(
                    EventQuery {
                        to_sequence: Some(through_sequence),
                        event_type: Some(event_type),
                        instance_id: Some(run.instance_id),
                        correlation_id: Some(run.correlation_id),
                        task_id: Some(run.task_id),
                        run_id: Some(run.run_id),
                        lease_id: Some(run.lease_id),
                        ..EventQuery::default()
                    },
                    0,
                    through_sequence,
                    2,
                )
                .map_err(|_| ledger_error("recover_policy_scheduling_terminal"))?,
        );
    }
    terminals.sort_by_key(PersistedEvent::sequence);
    match terminals.as_slice() {
        [] => Ok(None),
        [terminal] => Ok(Some(terminal.clone())),
        _ => Err(policy_admission_fatal(
            "policy_run_terminal_conflict",
            "recover_policy_scheduling_outcomes",
        )),
    }
}

pub(crate) fn insert_authoritative_policy_outcome(
    outcomes: &mut BTreeMap<(String, String), AuthoritativeSchedulingOutcome>,
    outcome: AuthoritativeSchedulingOutcome,
) -> RuntimeHostResult<()> {
    // The policy evaluator owns one current outcome observation per catalog task and instance.
    // Every value reaching this state projector has already been rebuilt from a caller-supplied
    // complete run identity by GlobalLedger. Sequence only orders those exact-run transitions; it
    // is never used to search history for a terminal that merely looks latest.
    let key = (
        outcome.identity().catalog_task_id().to_owned(),
        outcome.identity().instance_alias().to_owned(),
    );
    if let Some(existing) = outcomes.get(&key) {
        if existing == &outcome {
            return Ok(());
        }
        if existing.identity().terminal_sequence() >= outcome.identity().terminal_sequence() {
            return Err(policy_admission_fatal(
                "policy_outcome_replay_order_conflict",
                "commit_policy_scheduling_outcome",
            ));
        }
    } else if outcomes.len() >= MAX_AUTHORITATIVE_POLICY_OUTCOMES {
        return Err(policy_admission_fatal(
            "policy_scheduling_outcome_capacity_exceeded",
            "commit_policy_scheduling_outcome",
        ));
    }
    outcomes.insert(key, outcome);
    Ok(())
}

fn recovered_scheduled_task_runtime_ms(
    ledger: &GlobalLedger,
    through: u64,
    terminal: &PersistedEvent,
) -> RuntimeHostResult<u64> {
    let links = terminal.links();
    let requests = linked_policy_run_events(
        ledger,
        EventQuery {
            to_sequence: Some(through),
            event_type: Some(EventType::LabRequest),
            instance_id: links.instance_id().copied(),
            request_id: links.request_id().copied(),
            correlation_id: links.correlation_id().copied(),
            task_id: links.task_id().copied(),
            run_id: links.run_id().copied(),
            ..EventQuery::default()
        },
        through,
        2,
        "reconcile_policy_outcomes",
        |event| {
            event.event_type() == EventType::LabRequest
                && event.links().instance_id() == terminal.links().instance_id()
                && event.links().request_id() == terminal.links().request_id()
                && event.links().correlation_id() == terminal.links().correlation_id()
                && event.links().task_id() == terminal.links().task_id()
                && event.links().run_id() == terminal.links().run_id()
                && event.links().lease_id().is_none()
        },
    )?;
    let [request] = requests.as_slice() else {
        return Err(policy_admission_fatal(
            "policy_run_task_request_not_unique",
            "reconcile_policy_outcomes",
        ));
    };
    terminal
        .timestamp_unix_ms()
        .checked_sub(request.timestamp_unix_ms())
        .ok_or_else(|| {
            policy_admission_fatal(
                "policy_execution_clock_regressed",
                "reconcile_policy_outcomes",
            )
        })
}

fn policy_recovery_outcome_matches(
    outcome: &PolicyExecutionOutcome,
    input: &PolicyExecutionInput,
) -> bool {
    match (outcome, input) {
        (PolicyExecutionOutcome::Succeeded { .. }, PolicyExecutionInput::Succeeded) => true,
        (
            PolicyExecutionOutcome::Failed { failure },
            PolicyExecutionInput::Failed { error_code, class },
        ) => {
            !failure.reported_success
                && failure.error_code == *error_code
                && failure.original_class == *class
        }
        _ => false,
    }
}

/// A recorded execution always gives its pair a settlement; its absence is a host invariant
/// failure.
fn policy_settlement_missing() -> RuntimeHostError {
    policy_admission_fatal(
        "policy_settlement_missing",
        "record_policy_settlement_facts",
    )
}

fn policy_execution_severity(data: &PolicyExecutionEventData) -> EventSeverity {
    match &data.outcome {
        actingcommand_contract::PolicyExecutionOutcome::Succeeded { .. } => EventSeverity::Info,
        actingcommand_contract::PolicyExecutionOutcome::Failed { failure }
            if failure.effective_class
                == actingcommand_contract::PolicyFailureClass::Recoverable =>
        {
            EventSeverity::Warning
        }
        actingcommand_contract::PolicyExecutionOutcome::Failed { .. } => EventSeverity::Error,
    }
}
