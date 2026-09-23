// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

fn validate_static_fact_pool_authority(
    catalog: &actingcommand_policy::CompiledCatalog,
    facts: &EvaluationFacts,
    resources: &EvaluationResources,
    operation: &'static str,
) -> RuntimeHostResult<()> {
    for pool in &catalog.catalog().pools.pools {
        if pool.value_source.is_static() {
            continue;
        }
        let actingcommand_policy::ObservationRef::Fact { fact_key } = &pool.observation else {
            return Err(policy_admission_request(
                "policy_pool_binding_invalid",
                operation,
            ));
        };
        if resources.pools.iter().any(|value| value.pool_id == pool.id)
            || facts
                .facts
                .iter()
                .any(|fact| fact.scope == pool.scope && fact.fact_key == *fact_key)
        {
            return Err(policy_admission_request(
                "policy_pool_authority_conflict",
                operation,
            ));
        }
    }
    Ok(())
}

#[derive(Clone)]
struct TrustedPolicyDispatch {
    intent: DispatchIntent,
    reason_chain: DecisionReasonChain,
    observed_monotonic_ms: u64,
}

#[derive(Default)]
pub(super) struct TrustedPolicyDispatchStore {
    entries: BTreeMap<String, TrustedPolicyDispatch>,
    order: VecDeque<String>,
}

impl TrustedPolicyDispatchStore {
    fn record_cycle(
        &mut self,
        cycle: &PolicyCycle,
        observed_monotonic_ms: u64,
    ) -> RuntimeHostResult<()> {
        let Some(evaluation) = &cycle.evaluation else {
            return Ok(());
        };
        for intent in &cycle.pending_dispatch_intents {
            let reason_chain = evaluation
                .reason_chains
                .iter()
                .find(|reason| reason.id == intent.reason_chain_id)
                .ok_or_else(|| {
                    policy_admission_fatal(
                        "policy_reason_chain_missing",
                        "record_trusted_policy_dispatch",
                    )
                })?;
            let trusted = TrustedPolicyDispatch {
                intent: intent.clone(),
                reason_chain: reason_chain.clone(),
                observed_monotonic_ms,
            };
            if let Some(existing) = self.entries.get(&intent.decision_id) {
                if existing.intent != trusted.intent
                    || existing.reason_chain != trusted.reason_chain
                {
                    return Err(policy_admission_fatal(
                        "policy_decision_identity_conflict",
                        "record_trusted_policy_dispatch",
                    ));
                }
                continue;
            }
            self.order.push_back(intent.decision_id.clone());
            self.entries.insert(intent.decision_id.clone(), trusted);
        }
        while self.order.len() > MAX_TRUSTED_POLICY_DISPATCHES {
            if let Some(expired) = self.order.pop_front() {
                self.entries.remove(&expired);
            }
        }
        Ok(())
    }

    fn authorize(
        &self,
        intent: &DispatchIntent,
        reason_chain: &DecisionReasonChain,
    ) -> RuntimeHostResult<TrustedPolicyDispatch> {
        let trusted = self.entries.get(&intent.decision_id).ok_or_else(|| {
            policy_admission_request(
                "policy_decision_not_host_evaluated",
                "authorize_policy_dispatch",
            )
        })?;
        if trusted.intent != *intent || trusted.reason_chain != *reason_chain {
            return Err(policy_admission_request(
                "policy_trusted_context_mismatch",
                "authorize_policy_dispatch",
            ));
        }
        Ok(trusted.clone())
    }
}

struct PolicyAdmissionAppender<'a> {
    ledger: &'a GlobalLedger,
    initial_fact_gate: RefCell<Option<MutexGuard<'a, ()>>>,
}

impl<'a> PolicyAdmissionAppender<'a> {
    fn new(ledger: &'a GlobalLedger, initial_fact_gate: MutexGuard<'a, ()>) -> Self {
        Self {
            ledger,
            initial_fact_gate: RefCell::new(Some(initial_fact_gate)),
        }
    }
}

impl EventAppender for PolicyAdmissionAppender<'_> {
    fn append_durable(
        &self,
        draft: actingcommand_contract::SanitizedEventDraft,
    ) -> actingcommand_ledger::GlobalLedgerResult<PersistedEvent> {
        let event = self.ledger.append(draft)?;
        self.initial_fact_gate.borrow_mut().take();
        Ok(event)
    }
}

impl HostShared {
    pub(super) fn evaluate_policy_cycle(
        &self,
        trigger: PolicyTrigger,
    ) -> RuntimeHostResult<PolicyCycle> {
        let sample = self.runtime_clock_sample()?;
        let time = EvaluationTime {
            unix_ms: sample.unix_ms,
            monotonic_ms: sample.monotonic_ms,
        };
        self.evaluate_policy_cycle_authoritative(time, None, trigger, sample.monotonic_ms)
    }

    #[cfg(test)]
    pub(super) fn evaluate_policy_cycle_with_test_inputs(
        &self,
        facts: &EvaluationFacts,
        resources: &EvaluationResources,
        time: EvaluationTime,
        seed: u64,
        trigger: PolicyTrigger,
    ) -> RuntimeHostResult<PolicyCycle> {
        {
            let _gate = lock(&self.fact_write_gate, "set_test_policy_inputs")?;
            let inputs = PolicyInputSnapshot::new(facts.clone(), resources.clone());
            self.validate_policy_input_authority(&inputs, "set_test_policy_inputs")?;
            self.synchronize_fact_inputs_under_gate()?;
            // This existing replacement seam now replaces committed program
            // facts too. Explicit invalidation preserves the store's same-time
            // rejection rule without manufacturing a newer observation time.
            for instance in &facts.instances {
                use actingcommand_contract::{
                    CONFIG_POLICY_INSTANCE_IDENTITY_KEY, CONFIG_POLICY_INSTANCE_KEY,
                    CONFIG_POLICY_INSTANCE_SEEDED_KEY,
                };
                let scope = self.policy_instance_scope(&instance.instance_id)?;
                let now = self.clock.sample()?.unix_ms;
                if lock(&self.runtime_facts, "set_test_policy_inputs")?
                    .store
                    .get(&scope, CONFIG_POLICY_INSTANCE_SEEDED_KEY)
                    .is_none()
                {
                    self.record_runtime_fact_under_gate(RuntimeFactRecord {
                        scope: scope.clone(),
                        key: CONFIG_POLICY_INSTANCE_SEEDED_KEY.into(),
                        value: actingcommand_contract::FactValue::Boolean(true),
                        observed_at_unix_ms: now,
                        source: OriginModule::Runtime,
                        ttl_ms: None,
                    })?;
                }
                let mut identity = instance.clone();
                identity.available = false;
                identity.unavailable_reason = None;
                identity.capability_operation_ids.clear();
                identity.preferred_task_ids.clear();
                for (key, value) in [
                    (CONFIG_POLICY_INSTANCE_IDENTITY_KEY, identity),
                    (CONFIG_POLICY_INSTANCE_KEY, instance.clone()),
                ] {
                    let desired =
                        runtime_facts::policy_instance_record(scope.clone(), key, &value, now)?;
                    let prior = lock(&self.runtime_facts, "set_test_policy_inputs")?
                        .store
                        .get(&scope, key)
                        .cloned();
                    if let Some(prior) = prior {
                        if prior.value == desired.value && prior.ttl_ms == desired.ttl_ms {
                            continue;
                        }
                        self.invalidate_runtime_fact_under_gate(
                            &scope,
                            key,
                            RuntimeFactInvalidationReason::Operator,
                        )?;
                    }
                    self.record_runtime_fact_under_gate(desired)?;
                }
            }
            *lock(&self.policy_inputs, "set_test_policy_inputs")? = Some(inputs);
        }
        self.evaluate_policy_cycle_authoritative(time, Some(seed), trigger, self.monotonic_ms()?)
    }

    pub(super) fn evaluate_policy_cycle_authoritative(
        &self,
        time: EvaluationTime,
        seed: Option<u64>,
        trigger: PolicyTrigger,
        observed_monotonic_ms: u64,
    ) -> RuntimeHostResult<PolicyCycle> {
        if trigger == PolicyTrigger::Reconciliation {
            self.reconcile_pending_policy_settlements()?;
        }
        let _detection_gate = lock(&self.detection_write_gate, "plan_policy_detection")?;
        let procedure_manifest = lock(&self.procedure_manifest, "read_procedure_manifest")?
            .clone()
            .ok_or_else(|| {
                policy_admission_request("procedure_manifest_unconfigured", "evaluate_policy_cycle")
            })?;
        let (outcome_keys, facts, resources) = {
            let _outcome_gate = lock(&self.policy_outcome_gate, "snapshot_policy_outcome_state")?;
            let outcome_keys =
                lock(&self.policy, "read_policy_outcome_keys")?.outcome_key_snapshot()?;
            let _gate = lock(&self.fact_write_gate, "project_policy_facts")?;
            let (facts, resources) = self.project_authoritative_policy_inputs_under_gate(
                "evaluate_policy_cycle",
                &outcome_keys,
                None,
            )?;
            (outcome_keys, facts, resources)
        };
        let workloads = lock(&self.policy, "read_policy_performance_workloads")?
            .active_performance_workloads()?;
        let mut controlled_resources = resources;
        lock(
            &self.performance_control,
            "apply_policy_performance_control",
        )?
        .apply_to_resources(&mut controlled_resources.hosts, &workloads)?;
        let seed = match seed {
            Some(seed) => seed,
            None => runtime_policy_seed(&facts.fact_snapshot_id, time, self.owner_epoch)?,
        };
        let cycle = {
            let mut policy = lock(&self.policy, "evaluate_policy_cycle")?;
            policy.validate_outcome_key_snapshot(&outcome_keys)?;
            policy.evaluate(
                &facts,
                &controlled_resources,
                PolicyEvaluationContext {
                    procedure_manifest: &procedure_manifest,
                    time,
                    seed,
                    trigger,
                    sampled_at_monotonic_ms: observed_monotonic_ms,
                },
            )?
        };
        for signal in &cycle.detection_planning_signals {
            self.record_policy_planning_signal(signal.clone())?;
        }
        lock(
            &self.trusted_policy_dispatches,
            "record_trusted_policy_dispatches",
        )?
        .record_cycle(&cycle, observed_monotonic_ms)?;
        Ok(cycle)
    }

    #[cfg(test)]
    pub(super) fn replace_procedure_manifest_for_test(
        &self,
        procedure_manifest: ProcedureManifest,
    ) -> RuntimeHostResult<()> {
        let _gate = lock(&self.fact_write_gate, "replace_procedure_manifest_for_test")?;
        *lock(
            &self.procedure_manifest,
            "replace_procedure_manifest_for_test",
        )? = Some(procedure_manifest);
        Ok(())
    }

    pub(super) fn project_authoritative_policy_inputs_under_gate(
        &self,
        operation: &'static str,
        outcome_keys: &PolicyOutcomeKeySnapshot,
        as_of_ledger_position: Option<u64>,
    ) -> RuntimeHostResult<(EvaluationFacts, EvaluationResources)> {
        let latest_ledger_position = self.synchronize_fact_inputs_under_gate()?;
        let inputs = lock(&self.policy_inputs, "read_policy_inputs")?
            .clone()
            .ok_or_else(|| policy_admission_request("policy_inputs_unconfigured", operation))?;
        let ledger_position = match as_of_ledger_position {
            Some(position) if position == 0 || position > latest_ledger_position => {
                return Err(policy_admission_request(
                    "policy_input_position_unavailable",
                    operation,
                ));
            }
            Some(position) => position,
            None => latest_ledger_position,
        };
        #[cfg(test)]
        let ledger_position = if as_of_ledger_position.is_none() {
            match self
                .policy_outcome_projection_position_override
                .swap(0, Ordering::AcqRel)
            {
                0 => ledger_position,
                injected => injected,
            }
        } else {
            ledger_position
        };
        let mut base_facts = inputs.facts().clone();
        let (instances, program_revisions) = self.program_instances_at(
            &inputs.facts().instances,
            ledger_position,
            latest_ledger_position,
            as_of_ledger_position.is_some(),
        )?;
        base_facts.instances = instances;
        self.validate_policy_input_authority(
            &PolicyInputSnapshot::new(base_facts.clone(), inputs.resources().clone()),
            operation,
        )?;
        base_facts.tasks = lock(&self.policy, "project_policy_task_state")?
            .task_runtime_snapshots(ledger_position)?;
        base_facts.tasks.retain(|state| {
            base_facts
                .instances
                .iter()
                .any(|instance| instance.instance_id == state.instance_id)
        });
        let authoritative_outcomes = lock(
            &self.authoritative_policy_outcomes,
            "project_policy_scheduling_outcomes",
        )?;
        if base_facts
            .outcomes
            .iter()
            .any(|outcome| outcome_keys.keys.contains_key(&outcome.task_id))
        {
            return Err(policy_admission_request(
                "policy_outcome_authority_conflict",
                operation,
            ));
        }
        for (key, expected_run) in &outcome_keys.completed_runs {
            let Some(expected_keys) = outcome_keys.keys.get(&expected_run.catalog_task_id) else {
                continue;
            };
            if matches!(
                expected_run.execution_outcome,
                PolicyExecutionOutcome::Failed { .. }
            ) {
                if authoritative_outcomes.contains_key(key) {
                    return Err(RuntimeHostError::fatal(
                        "policy_outcome_failed_run_residual",
                        operation,
                        RuntimeErrorCode::RuntimeFatal,
                    ));
                }
                continue;
            }
            let outcome = authoritative_outcomes.get(key).ok_or_else(|| {
                RuntimeHostError::request(
                    "outcome_projection_not_ready",
                    operation,
                    RuntimeErrorCode::RuntimeUnavailable,
                )
            })?;
            let identity = outcome.identity();
            if !completed_run_matches_outcome(expected_run, outcome)
                || !expected_keys.contains(outcome.disposition().outcome_key())
            {
                return Err(RuntimeHostError::request(
                    "outcome_projection_not_ready",
                    operation,
                    RuntimeErrorCode::RuntimeUnavailable,
                ));
            }
            if !base_facts
                .instances
                .iter()
                .any(|instance| instance.instance_id == identity.instance_alias())
            {
                continue;
            }
            if ledger_position < identity.terminal_sequence() {
                return Err(RuntimeHostError::request(
                    "outcome_projection_not_ready",
                    operation,
                    RuntimeErrorCode::RuntimeUnavailable,
                ));
            }
            #[cfg(test)]
            if self
                .policy_outcome_projection_failures
                .swap(0, Ordering::AcqRel)
                != 0
            {
                return Err(RuntimeHostError::fatal(
                    "policy_outcome_projection_injected_failure",
                    operation,
                    RuntimeErrorCode::RuntimeFatal,
                ));
            }
            let projected = self
                .ledger
                .project_scheduling_outcomes(identity.clone(), ledger_position)
                .map_err(|error| {
                    if error.code() == "outcome_projection_not_ready"
                        || error.code() == "outcome_projection_position_invalid"
                    {
                        RuntimeHostError::request(
                            "outcome_projection_not_ready",
                            operation,
                            RuntimeErrorCode::RuntimeUnavailable,
                        )
                    } else {
                        ledger_error("project_policy_scheduling_outcome")
                    }
                })?;
            if projected.outcome() != outcome {
                return Err(RuntimeHostError::fatal(
                    "policy_outcome_projection_mismatch",
                    operation,
                    RuntimeErrorCode::RuntimeFatal,
                ));
            }
            validate_completed_run_admission_request(
                &self.ledger,
                expected_run,
                identity.terminal_sequence(),
            )?;
            base_facts.outcomes.push(ObservedOutcome {
                task_id: identity.catalog_task_id().to_owned(),
                instance_id: identity.instance_alias().to_owned(),
                outcome_key: outcome.disposition().outcome_key().to_owned(),
                value: PolicyFactValue::Boolean(true),
                observed_at_unix_ms: outcome.terminal_timestamp_unix_ms(),
                expires_at_unix_ms: None,
                activity_window_id: Some(expected_run.activity_window_id.clone()),
            });
        }
        let fact_store = lock(&self.facts, "project_policy_facts")?;
        let historical;
        let fact_projection = if ledger_position == latest_ledger_position {
            &*fact_store
        } else {
            historical = fact_store.at_position(&self.ledger, ledger_position)?;
            &historical
        };
        let catalog = lock(&self.policy, "project_fact_pool_catalog")?.active_loaded();
        if let Some(catalog) = &catalog {
            validate_static_fact_pool_authority(
                catalog.compiled(),
                inputs.facts(),
                inputs.resources(),
                operation,
            )?;
        }
        let facts = fact_projection.overlay_policy_facts(
            &base_facts,
            inputs.resources(),
            ledger_position,
        )?;
        let resources = if let Some(catalog) = catalog {
            fact_projection.validate_pool_sources(catalog.compiled(), |scope| {
                self.fact_scope_instances(scope, &base_facts.instances)
            })?;
            actingcommand_policy::project_fact_pools(catalog.compiled(), &facts, inputs.resources())
        } else {
            inputs.resources().clone()
        };
        let mut facts =
            fact_projection.overlay_policy_facts(&base_facts, &resources, ledger_position)?;
        // Only records consumed by the instance projection are authority inputs.
        // Snapshot sampling time and unrelated program facts are not revisions.
        let identity =
            serde_json::to_vec(&(&facts.fact_snapshot_id, &program_revisions)).map_err(|_| {
                policy_admission_fatal("policy_instance_identity_encode_failed", operation)
            })?;
        facts.fact_snapshot_id = format!("snapshot:policy-fact:{:x}", Sha256::digest(identity));
        Ok((facts, resources))
    }

    pub(super) fn validate_policy_input_authority(
        &self,
        inputs: &PolicyInputSnapshot,
        operation: &'static str,
    ) -> RuntimeHostResult<()> {
        let registered = lock(
            &self.registered_instances,
            "validate_policy_instance_metadata",
        )?;
        let registered_aliases = registered
            .values()
            .map(|instance| instance.instance_alias.as_str())
            .collect::<BTreeSet<_>>();
        let snapshot_aliases = inputs
            .facts()
            .instances
            .iter()
            .map(|instance| instance.instance_id.as_str())
            .collect::<BTreeSet<_>>();
        if registered_aliases != snapshot_aliases {
            return Err(policy_admission_request(
                "policy_instance_metadata_untrusted",
                operation,
            ));
        }
        let host_ids = inputs
            .resources()
            .hosts
            .iter()
            .map(|host| host.host_id.as_str())
            .collect::<BTreeSet<_>>();
        if inputs
            .facts()
            .instances
            .iter()
            .any(|instance| !host_ids.contains(instance.host_id.as_str()))
        {
            return Err(policy_admission_request(
                "policy_resource_metadata_untrusted",
                operation,
            ));
        }
        Ok(())
    }

    pub(super) fn project_policy_forward(
        &self,
        facts: &EvaluationFacts,
        resources: &EvaluationResources,
        time: EvaluationTime,
        seed: u64,
        config: ForwardProjectionConfig,
    ) -> RuntimeHostResult<ForwardProjection> {
        let declared_facts = facts;
        let (facts, fact_projection, scope_instances) = {
            let _gate = lock(&self.fact_write_gate, "project_forward_facts")?;
            if self.fact_projection_failed.load(Ordering::Acquire)
                || self.lifecycle_append_failed.load(Ordering::Acquire)
            {
                return Err(ledger_error("fact_projection_unavailable"));
            }
            // Forward projection remains read-only: synchronize temporary copies
            // to one cut without acknowledging or appending derived invalidations.
            let mut fact_projection = lock(&self.facts, "project_forward_facts")?.clone();
            fact_projection
                .synchronize(&self.ledger)
                .inspect_err(|error| {
                    if error.is_fatal() {
                        self.fact_projection_failed.store(true, Ordering::Release);
                    }
                })?;
            let ledger_position = fact_projection.applied_position();
            let mut program_projection =
                lock(&self.runtime_facts, "project_forward_facts")?.clone();
            program_projection
                .synchronize_to(&self.ledger, ledger_position)
                .inspect_err(|error| {
                    if error.is_fatal() {
                        self.fact_projection_failed.store(true, Ordering::Release);
                    }
                })?;
            let inputs = lock(&self.policy_inputs, "project_forward_facts")?;
            let scope_instances = if let Some(inputs) = inputs.as_ref() {
                self.project_program_instances(
                    &inputs.facts().instances,
                    &program_projection.store,
                    self.clock.sample()?.unix_ms,
                )?
                .0
            } else {
                Vec::new()
            };
            let facts =
                fact_projection.overlay_external_policy_facts(facts, resources, ledger_position)?;
            (facts, fact_projection, scope_instances)
        };
        let (catalog, workloads) = {
            let policy = lock(&self.policy, "project_forward_catalog")?;
            let catalog = policy.active_loaded().ok_or_else(|| {
                RuntimeHostError::request(
                    "policy_catalog_unavailable",
                    "project_policy_forward",
                    RuntimeErrorCode::InvalidRequest,
                )
            })?;
            (catalog, policy.active_performance_workloads()?)
        };
        validate_static_fact_pool_authority(
            catalog.compiled(),
            declared_facts,
            resources,
            "project_policy_forward",
        )?;
        fact_projection.validate_pool_sources(catalog.compiled(), |scope| {
            self.fact_scope_instances(scope, &scope_instances)
        })?;
        let mut resources =
            actingcommand_policy::project_fact_pools(catalog.compiled(), &facts, resources);
        lock(
            &self.performance_control,
            "apply_forward_performance_control",
        )?
        .apply_to_resources(&mut resources.hosts, &workloads)?;
        project_forward(catalog.compiled(), &facts, &resources, time, seed, config).map_err(
            |error| {
                RuntimeHostError::request(
                    error.code(),
                    "project_policy_forward",
                    RuntimeErrorCode::InvalidRequest,
                )
            },
        )
    }

    pub(super) fn admit_policy_dispatch(
        &self,
        intent: &DispatchIntent,
        reason_chain: &DecisionReasonChain,
        context: &PolicyAdmissionContext,
        task_request: Option<&ContainedTaskRequest>,
    ) -> RuntimeHostResult<PolicyDispatchAdmission> {
        let mut rejection = None;
        let result = (|| {
            {
                let policy = lock(&self.policy, "validate_policy_dispatch")?;
                if let Some(replay) = policy.replay_admission(intent, reason_chain)? {
                    return Ok(replay);
                }
            }
            let trusted = lock(
                &self.trusted_policy_dispatches,
                "authorize_trusted_policy_dispatch",
            )?
            .authorize(intent, reason_chain)?;
            if context.fact_ledger_position != trusted.intent.input_ledger_position
                || context.fact_snapshot_id != trusted.intent.fact_snapshot_id
                || context.fencing_owner_epoch != self.owner_epoch
            {
                return Err(policy_admission_request(
                    "policy_admission_context_untrusted",
                    "admit_policy_dispatch",
                ));
            }
            let elapsed_ms = self
                .monotonic_ms()?
                .checked_sub(trusted.observed_monotonic_ms)
                .ok_or_else(|| {
                    policy_admission_fatal(
                        "policy_admission_clock_regressed",
                        "admit_policy_dispatch",
                    )
                })?;
            let now_unix_ms = trusted
                .intent
                .prerequisites
                .evaluated_at_unix_ms
                .checked_add(elapsed_ms)
                .ok_or_else(|| {
                    policy_admission_fatal(
                        "policy_admission_clock_overflow",
                        "admit_policy_dispatch",
                    )
                })?;
            // Approval projection and dispatch admission share one order so a concurrent revocation
            // cannot appear in the ledger before a dispatch authorized by the superseded fact.
            let _governance_gate = lock(&self.governance_write_gate, "project_policy_approvals")?;
            let approval_fact_ids =
                match ApprovalProjection::recover(&self.ledger, Arc::clone(&self.state)) {
                    Ok(projection) => projection.active_for_dispatch(intent),
                    Err(error) => {
                        self.fatal.mark(error.clone())?;
                        return Err(error);
                    }
                };
            let authoritative_context = PolicyAdmissionContext {
                fact_ledger_position: trusted.intent.input_ledger_position,
                fact_snapshot_id: trusted.intent.fact_snapshot_id.clone(),
                approval_fact_ids,
                fencing_owner_epoch: self.owner_epoch,
                now_unix_ms,
            };
            let context = &authoritative_context;
            let mut gate_error = match lock(
                &self.performance_control,
                "gate_policy_performance_dispatch",
            )?
            .gate_dispatch(
                &intent.instance_id,
                intent.prerequisites.urgency_milli,
                context.now_unix_ms,
            )? {
                PerformanceDispatchGate::Allowed => None,
                PerformanceDispatchGate::Deferred {
                    reason,
                    deadline_disposition,
                    event,
                } => {
                    if let Some(event) = event {
                        self.record_performance_events(&[
                            PerformanceSemanticEvent::BalanceChanged(event),
                        ])?;
                    }
                    let code = if deadline_disposition
                        == Some(
                            actingcommand_contract::PerformanceDeadlineDisposition::CapacityFailure,
                        ) {
                        "performance_capacity_deadline_conflict"
                    } else {
                        reason
                    };
                    Some(RuntimeHostError::request(
                        code,
                        "admit_policy_dispatch",
                        RuntimeErrorCode::InvalidRequest,
                    ))
                }
            };
            if gate_error.is_none()
                && let Err(error) = self.admit_capacity()
            {
                if error.is_fatal() {
                    return Err(error);
                }
                gate_error = Some(error);
            }
            let resolved = self
                .resolve_instance(&intent.instance_id)
                .map_err(|failure| *failure.error)?;
            let request_id = self
                .events
                .issuer()
                .mint_request_id()
                .map_err(|_| policy_id_error("issue_policy_request_id"))?;
            let correlation_id = self
                .events
                .issuer()
                .mint_correlation_id()
                .map_err(|_| policy_id_error("issue_policy_correlation_id"))?;
            let holder = self
                .events
                .issuer()
                .mint_holder_id()
                .map_err(|_| policy_id_error("issue_policy_holder_id"))?;
            let task_id = self
                .events
                .issuer()
                .mint_task_id()
                .map_err(|_| policy_id_error("issue_policy_task_id"))?;
            let run_id = self
                .events
                .issuer()
                .mint_run_id()
                .map_err(|_| policy_id_error("issue_policy_run_id"))?;
            let run_links = RuntimeRunLinks::new(task_id, run_id);
            let holder_id = *holder.transport();
            let request = RuntimeRequest::new(
                request_id,
                correlation_id,
                None,
                EventActor::Agent,
                EventSource::Adapter,
                context.now_unix_ms,
                RuntimeOperation::acquire_lease(intent.instance_id.clone(), holder),
            )
            .map_err(|_| policy_contract_error("build_policy_runtime_request"))?;
            let validated = request
                .validate()
                .map_err(|_| policy_contract_error("validate_policy_runtime_request"))?;
            let connection_id = ConnectionId::new(POLICY_CONNECTION_VALUE)
                .map_err(|error| RuntimeHostError::scheduler("build_policy_connection", &error))?;
            let action_id = self.events.action_id()?;
            let links = run_links.apply(self.events.request_links(
                &validated,
                Some(resolved.instance_id()),
                None,
                Some(action_id),
            ));
            let data = policy_event_data(intent, reason_chain)?;
            let event = self.events.draft(
                EventSeverity::Info,
                EventSource::Scheduler,
                OriginModule::Policy,
                EventActor::Scheduler,
                links.clone(),
                PolicyPayloadDraft::dispatch_intent(data.clone(), AuditInput::new()),
            )?;
            let event = self.events.sanitize(event)?;
            let plan = CriticalEventPlan::new(CriticalOperation::PolicyDispatch, event)
                .map_err(|_| critical_plan_error())?;
            let (outcome_keys, current_facts, fact_gate) = {
                let _outcome_gate =
                    lock(&self.policy_outcome_gate, "snapshot_policy_outcome_state")?;
                let outcome_keys =
                    lock(&self.policy, "read_policy_outcome_keys")?.outcome_key_snapshot()?;
                if outcome_keys.generation.as_ref().is_none_or(|generation| {
                    generation.catalog_hash() != intent.catalog_hash
                        || generation.catalog_version() != intent.catalog_version
                }) {
                    return Err(policy_admission_request(
                        "catalog_active_generation_changed",
                        "admit_policy_dispatch",
                    ));
                }
                let fact_gate = lock(&self.fact_write_gate, "validate_policy_fact_freshness")?;
                let (current_facts, _) = self.project_authoritative_policy_inputs_under_gate(
                    "admit_policy_dispatch",
                    &outcome_keys,
                    None,
                )?;
                (outcome_keys, current_facts, fact_gate)
            };
            if current_facts.fact_snapshot_id != trusted.intent.fact_snapshot_id {
                return Err(policy_admission_request(
                    "policy_facts_stale",
                    "admit_policy_dispatch",
                ));
            }
            lock(&self.procedure_manifest, "validate_procedure_manifest")?
                .as_ref()
                .ok_or_else(|| {
                    policy_admission_request(
                        "procedure_manifest_unconfigured",
                        "admit_policy_dispatch",
                    )
                })?
                .validate_intent(intent, "admit_policy_dispatch")?;
            let appender = PolicyAdmissionAppender::new(&self.ledger, fact_gate);
            let success_links = links.clone();
            let failure_links = links;
            let success_data = data.clone();
            let failure_data = data;
            let result = execute_critical(
                &appender,
                self.events.fingerprinter(),
                plan,
                || {
                    #[cfg(test)]
                    policy_crash_test_barrier("after_policy_intent");
                    if let Some(error) = gate_error.clone() {
                        return CriticalActionReport::Failed {
                            error: RequestFailure::request(
                                error,
                                RuntimeReceiptState::Denied,
                                None,
                            ),
                            effect: EffectDisposition::NotPerformed,
                        };
                    }
                    let ledger_high_watermark = match self.ledger.latest_sequence() {
                        Ok(position) => position,
                        Err(_) => {
                            return CriticalActionReport::Failed {
                                error: RequestFailure::poison_without_terminal(ledger_error(
                                    "read_policy_ledger_position",
                                )),
                                effect: EffectDisposition::NotPerformed,
                            };
                        }
                    };
                    let mut policy = match lock(&self.policy, "validate_policy_dispatch") {
                        Ok(policy) => policy,
                        Err(error) => {
                            return CriticalActionReport::Failed {
                                error: RequestFailure::poison_without_terminal(error),
                                effect: EffectDisposition::NotPerformed,
                            };
                        }
                    };
                    if let Err(error) = policy.validate_outcome_key_snapshot(&outcome_keys) {
                        return CriticalActionReport::Failed {
                            error: RequestFailure::request(
                                error,
                                RuntimeReceiptState::Denied,
                                None,
                            ),
                            effect: EffectDisposition::NotPerformed,
                        };
                    }
                    let catalog = match policy.validate_dispatch(
                        intent,
                        reason_chain,
                        context,
                        self.owner_epoch,
                        ledger_high_watermark,
                    ) {
                        Ok(catalog) => catalog,
                        Err(error) => {
                            let failure = if error.is_fatal() {
                                RequestFailure::poison_without_terminal(error)
                            } else {
                                RequestFailure::request(error, RuntimeReceiptState::Denied, None)
                            };
                            return CriticalActionReport::Failed {
                                error: failure,
                                effect: EffectDisposition::NotPerformed,
                            };
                        }
                    };
                    let admission_record = match policy
                        .preview_admission(intent, context.now_unix_ms)
                    {
                        Ok(record) => record,
                        Err(error) => {
                            let failure = if error.is_fatal() {
                                RequestFailure::poison_without_terminal(error)
                            } else {
                                RequestFailure::request(error, RuntimeReceiptState::Denied, None)
                            };
                            return CriticalActionReport::Failed {
                                error: failure,
                                effect: EffectDisposition::NotPerformed,
                            };
                        }
                    };
                    let lease_ttl_ms = task_request
                        .map(|task_request| {
                            task_request.validate().map_err(|_| {
                                RequestFailure::request(
                                    policy_admission_request(
                                        "policy_task_request_invalid",
                                        "admit_policy_dispatch",
                                    ),
                                    RuntimeReceiptState::Denied,
                                    None,
                                )
                            })?;
                            if intent.package_digest.as_ref()
                                != Some(task_request.expected_sha256())
                            {
                                return Err(RequestFailure::request(
                                    policy_admission_request(
                                        "procedure_package_digest_mismatch",
                                        "admit_policy_dispatch",
                                    ),
                                    RuntimeReceiptState::Denied,
                                    None,
                                ));
                            }
                            self.contained_task_lease_ttl(task_request)
                        })
                        .transpose();
                    let lease_ttl_ms = match lease_ttl_ms {
                        Ok(ttl) => ttl,
                        Err(error) => {
                            return CriticalActionReport::Failed {
                                error,
                                effect: EffectDisposition::NotPerformed,
                            };
                        }
                    };
                    let admission = self.acquire_lease(RuntimeLeaseAcquisition {
                        request: &validated,
                        request_id: request.request_id(),
                        instance_alias: &intent.instance_id,
                        holder_id,
                        connection_id,
                        run_links: Some(run_links),
                        lease_ttl_ms,
                    });
                    match admission {
                        Ok(success) => match success.result {
                            RuntimeResult::LeaseGranted { token } => {
                                #[cfg(test)]
                                policy_crash_test_barrier("after_lease_grant");
                                if let Err(error) =
                                    policy.commit_admission(intent, &admission_record)
                                {
                                    return CriticalActionReport::Failed {
                                        error: RequestFailure::poison_without_terminal(error),
                                        effect: EffectDisposition::Indeterminate,
                                    };
                                }
                                #[cfg(test)]
                                policy_crash_test_barrier("after_budget_commit");
                                CriticalActionReport::Succeeded {
                                    value: (token, catalog, admission_record),
                                    effect: DefiniteEffectDisposition::Performed,
                                }
                            }
                            _ => CriticalActionReport::Failed {
                                error: RequestFailure::poison_without_terminal(
                                    RuntimeHostError::fatal(
                                        "policy_lease_result_invalid",
                                        "admit_policy_dispatch",
                                        RuntimeErrorCode::RuntimeFatal,
                                    ),
                                ),
                                effect: EffectDisposition::Indeterminate,
                            },
                        },
                        Err(error) => {
                            let effect = if error.poison_runtime {
                                EffectDisposition::Indeterminate
                            } else {
                                EffectDisposition::NotPerformed
                            };
                            CriticalActionReport::Failed { error, effect }
                        }
                    }
                },
                |(_, _, admission), _| {
                    self.events
                        .draft(
                            EventSeverity::Info,
                            EventSource::Scheduler,
                            OriginModule::Policy,
                            EventActor::Scheduler,
                            success_links,
                            PolicyPayloadDraft::dispatch_admitted(
                                success_data,
                                admission.clone(),
                                AuditInput::new(),
                            ),
                        )
                        .map_err(|_| {
                            actingcommand_contract::SanitizationError::fingerprinter_failure()
                        })
                },
                |failure, effect| {
                    self.events
                        .draft(
                            EventSeverity::Error,
                            EventSource::Scheduler,
                            OriginModule::Policy,
                            EventActor::Scheduler,
                            failure_links,
                            PolicyPayloadDraft::dispatch_rejected_with_reason(
                                failure_data,
                                effect,
                                failure.error.policy_rejection(),
                                AuditInput::new(),
                            ),
                        )
                        .map_err(|_| {
                            actingcommand_contract::SanitizationError::fingerprinter_failure()
                        })
                },
            );
            if let Err(refresh) = self.refresh_policy_dispatches() {
                return Err(match result {
                    Err(CriticalExecutionError::Action { error, outcome, .. }) => {
                        rejection = Some((
                            outcome,
                            self.events.request_links(
                                &validated,
                                Some(resolved.instance_id()),
                                None,
                                None,
                            ),
                        ));
                        (*error.error).with_complete_failure(
                            crate::error::RuntimeFailureRelation::AdmissionRecord,
                            refresh,
                        )
                    }
                    Err(error) => critical_execution_error(&error).with_complete_failure(
                        crate::error::RuntimeFailureRelation::AdmissionRecord,
                        refresh,
                    ),
                    Ok(_) => refresh,
                });
            }
            match result {
                Ok(receipt) => {
                    let started_at_monotonic_ms = self.monotonic_ms()?;
                    let (token, catalog, admission) = receipt.into_value();
                    let clock = PolicyDispatchClock::live(
                        admission.activity.admitted_at_unix_ms,
                        started_at_monotonic_ms,
                    );
                    if lock(&self.policy_dispatch_clocks, "record_policy_dispatch_start")?
                        .insert(intent.decision_id.clone(), clock)
                        .is_some()
                    {
                        let error = policy_admission_fatal(
                            "policy_dispatch_clock_identity_conflict",
                            "record_policy_dispatch_start",
                        );
                        self.fatal.mark(error.clone())?;
                        return Err(error);
                    }
                    Ok(PolicyDispatchAdmission::Granted {
                        context: Box::new(PolicyRunContext::new(
                            request,
                            correlation_id,
                            run_id,
                            task_id,
                            catalog,
                            token,
                            admission,
                            intent.clone(),
                            reason_chain.clone(),
                        )?),
                    })
                }
                Err(CriticalExecutionError::Action { error, outcome, .. }) => {
                    rejection = Some((
                        outcome,
                        self.events.request_links(
                            &validated,
                            Some(resolved.instance_id()),
                            None,
                            None,
                        ),
                    ));
                    if error.poison_runtime {
                        self.fatal.mark((*error.error).clone())?;
                    }
                    Err(*error.error)
                }
                Err(error) => {
                    let error = critical_execution_error(&error);
                    self.fatal.mark(error.clone())?;
                    Err(error)
                }
            }
        })();
        self.record_policy_admission_result(intent, result, rejection)
    }

    pub(super) fn record_policy_admission_result(
        &self,
        intent: &DispatchIntent,
        result: RuntimeHostResult<PolicyDispatchAdmission>,
        rejection: Option<(Box<PersistedEvent>, EventLinksDraft)>,
    ) -> RuntimeHostResult<PolicyDispatchAdmission> {
        match result {
            Err(error) if !error.is_fatal() || rejection.is_some() => {
                let recorded = match rejection {
                    Some((outcome, links)) => self.record_required_failure(&error, &outcome, links),
                    None => self.append_lifecycle_failure(
                        RuntimeLifecycleFailureStage::OperationCleanup,
                        RuntimeLifecycleFailure::PolicyAdmission {
                            error: &error,
                            decision_id: &intent.decision_id,
                        },
                        EventLinksDraft::default(),
                        None,
                    ),
                };
                match recorded {
                    Ok(()) => Err(error),
                    Err(writer) => {
                        let complete = error
                            .with_complete_failure(
                                crate::error::RuntimeFailureRelation::AdmissionRecord,
                                writer,
                            )
                            .into_fatal();
                        match self.fatal.mark(complete.clone()) {
                            Ok(()) => Err(complete),
                            Err(mark) => Err(complete.with_complete_failure(
                                crate::error::RuntimeFailureRelation::LifecycleRecord,
                                mark,
                            )),
                        }
                    }
                }
            }
            result => result,
        }
    }

    pub(super) fn refresh_policy_dispatches(&self) -> RuntimeHostResult<()> {
        let result =
            lock(&self.policy, "recover_policy_dispatches")?.refresh_dispatches(&self.ledger);
        if let Err(error) = &result {
            self.fatal.mark(error.clone())?;
        }
        result
    }

    pub(super) fn pinned_policy_catalog(
        &self,
        decision_id: &str,
    ) -> RuntimeHostResult<Option<CatalogGeneration>> {
        Ok(lock(&self.policy, "read_pinned_policy_catalog")?.pinned_catalog(decision_id))
    }

    pub(super) fn project_policy_input_identity(
        &self,
        as_of_ledger_position: u64,
    ) -> Result<OperationSuccess, RequestFailure> {
        let identity = (|| {
            if as_of_ledger_position == 0 {
                return Err(RuntimeHostError::request(
                    "policy_input_position_unavailable",
                    "project_policy_input_identity",
                    RuntimeErrorCode::InvalidRequest,
                ));
            }
            let _outcome_gate = lock(
                &self.policy_outcome_gate,
                "snapshot_policy_input_identity_outcome_state",
            )?;
            let outcome_keys = lock(&self.policy, "read_policy_input_identity_outcome_keys")?
                .outcome_key_snapshot()?;
            let _fact_gate = lock(&self.fact_write_gate, "project_policy_input_identity_facts")?;
            let (facts, _) = self.project_authoritative_policy_inputs_under_gate(
                "project_policy_input_identity",
                &outcome_keys,
                Some(as_of_ledger_position),
            )?;
            RuntimePolicyInputIdentity::new(facts.ledger_position, facts.fact_snapshot_id).map_err(
                |_| {
                    RuntimeHostError::fatal(
                        "policy_input_identity_invalid",
                        "project_policy_input_identity",
                        RuntimeErrorCode::RuntimeFatal,
                    )
                },
            )
        })()
        .map_err(planning_request_failure)?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::PolicyInputIdentityProjected { identity },
        })
    }
}

fn policy_event_data(
    intent: &DispatchIntent,
    reason_chain: &DecisionReasonChain,
) -> RuntimeHostResult<PolicyDispatchEventData> {
    let package_digest = intent.package_digest.clone().ok_or_else(|| {
        policy_admission_fatal(
            "procedure_package_digest_missing",
            "build_policy_dispatch_event",
        )
    })?;
    let procedure_binding_digest = intent.procedure_binding_digest.clone().ok_or_else(|| {
        policy_admission_fatal(
            "procedure_binding_digest_missing",
            "build_policy_dispatch_event",
        )
    })?;
    Ok(PolicyDispatchEventData {
        decision_id: intent.decision_id.clone(),
        task_id: intent.task_id.clone(),
        instance_id: intent.instance_id.clone(),
        operation_id: intent.operation_id.clone(),
        package_digest,
        procedure_binding_digest,
        reason_chain_id: reason_chain.id.clone(),
        reasons: reason_chain
            .reasons
            .iter()
            .map(|reason| PolicyReasonRecord {
                code: reason.code.clone(),
                detail: reason.detail.clone(),
            })
            .collect(),
        catalog_hash: intent.catalog_hash.clone(),
        catalog_version: intent.catalog_version,
        input_ledger_position: intent.input_ledger_position,
        fact_snapshot_id: intent.fact_snapshot_id.clone(),
        approval_fact_ids: intent.approval_refs.clone(),
        urgency_milli: intent.prerequisites.urgency_milli,
    })
}

fn runtime_policy_seed(
    fact_snapshot_id: &str,
    time: EvaluationTime,
    owner_epoch: actingcommand_contract::OwnerEpoch,
) -> RuntimeHostResult<u64> {
    let bytes = serde_json::to_vec(&(fact_snapshot_id, time, owner_epoch)).map_err(|_| {
        RuntimeHostError::fatal(
            "policy_seed_encode_failed",
            "derive_policy_seed",
            RuntimeErrorCode::RuntimeFatal,
        )
    })?;
    let digest = Sha256::digest(bytes);
    let mut seed = [0_u8; 8];
    seed.copy_from_slice(&digest[..8]);
    Ok(u64::from_be_bytes(seed))
}
