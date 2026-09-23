// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

impl HostShared {
    pub(super) fn publish_fact(&self, record: FactRecord) -> RuntimeHostResult<EventId> {
        self.publish_facts(
            actingcommand_contract::FactObservation {
                records: vec![record],
            },
            None,
        )
    }

    pub(super) fn fact_scope_instances(
        &self,
        scope: &actingcommand_contract::FactScope,
    ) -> RuntimeHostResult<Vec<InstanceId>> {
        let inputs = lock(&self.policy_inputs, "bind_fact_scope")?;
        let registered = lock(&self.registered_instances, "bind_fact_scope")?;
        Ok(registered
            .values()
            .filter(|instance| match scope {
                actingcommand_contract::FactScope::Instance { instance_id } => {
                    instance_id == &instance.instance_alias
                }
                _ => inputs.as_ref().is_some_and(|inputs| {
                    inputs.facts().instances.iter().any(|context| {
                        context.instance_id == instance.instance_alias
                            && scope.matches(&InstanceFactContext {
                                instance_id: context.instance_id.clone(),
                                server_id: context.server_id.clone(),
                                game_id: context.game_id.clone(),
                            })
                    })
                }),
            })
            .map(|instance| instance.instance_id)
            .collect())
    }

    pub(super) fn publish_facts(
        &self,
        observation: actingcommand_contract::FactObservation,
        source_request: Option<&ValidatedRuntimeRequest<'_>>,
    ) -> RuntimeHostResult<EventId> {
        let result: RuntimeHostResult<EventId> = (|| {
            let _gate = lock(&self.fact_write_gate, "publish_fact")?;
            self.synchronize_fact_store_under_gate()?;
            if let Some(event_id) = lock(&self.facts, "publish_fact")?.preview_observation(
                &observation,
                self.clock.sample()?.unix_ms,
                &self.ledger,
            )? {
                return Ok(event_id);
            }
            let scope = &observation.records[0].scope;
            let inputs = lock(&self.policy_inputs, "bind_fact_scope")?;
            if inputs.as_ref().is_some_and(|inputs| {
                observation.records.iter().any(|record| {
                    inputs.facts().facts.iter().any(|fact| {
                        fact.scope == crate::fact_store::policy_scope(&record.scope)
                            && fact.fact_key == record.key
                    })
                })
            }) {
                return Err(policy_admission_request(
                    "policy_fact_authority_conflict",
                    "publish_facts",
                ));
            }
            drop(inputs);
            let scope_instances = self.fact_scope_instances(scope)?;
            lock(&self.facts, "validate_fact_input_boundary")?
                .validate_input_boundaries(&observation.records[0], &scope_instances)?;
            if scope_instances.is_empty()
                && observation.records[0].invalidate_on.iter().any(|event| {
                    matches!(event, EventType::InputCommitted | EventType::InputFailed)
                })
            {
                return Err(policy_admission_request(
                    "fact_invalidation_scope_unbound",
                    "publish_facts",
                ));
            }
            let payload =
                FactPayloadDraft::observation(observation, scope_instances, AuditInput::new())
                    .map_err(|_| {
                        policy_admission_request("fact_observation_invalid", "publish_facts")
                    })?;
            let links = match source_request {
                Some(request) => self.events.request_links(request, None, None, None),
                None => self.events.system_links()?,
            };
            let event = self.append_event_under_fact_gate(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::FactStore,
                EventActor::Runtime,
                links,
                payload,
            )?;
            self.synchronize_fact_store_under_gate()?;
            Ok(*event.event_id())
        })();
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.fatal.mark(error.clone())?;
        }
        result
    }

    pub(super) fn instance_fact_snapshot(
        &self,
        context: InstanceFactContext,
    ) -> RuntimeHostResult<InstanceFactSnapshot> {
        let _gate = lock(&self.fact_write_gate, "read_instance_fact_snapshot")?;
        self.synchronize_fact_store_under_gate()?;
        let ledger_position = self
            .ledger
            .latest_sequence()
            .map_err(|_| ledger_error("read_instance_fact_position"))?;
        lock(&self.facts, "read_instance_fact_snapshot")?.snapshot(context, ledger_position)
    }

    pub(super) fn append_event(
        &self,
        severity: EventSeverity,
        source: EventSource,
        module: OriginModule,
        actor: EventActor,
        links: EventLinksDraft,
        payload: impl Into<actingcommand_contract::EventPayloadDraft>,
    ) -> Result<PersistedEvent, RequestFailure> {
        self.append_event_raw(severity, source, module, actor, links, payload)
            .map_err(RequestFailure::poison_without_terminal)
    }

    pub(super) fn append_event_observed(
        &self,
        severity: EventSeverity,
        source: EventSource,
        module: OriginModule,
        actor: EventActor,
        links: EventLinksDraft,
        payload: impl Into<actingcommand_contract::EventPayloadDraft>,
    ) -> (
        Result<PersistedEvent, RequestFailure>,
        task_timing::AppendObservation,
    ) {
        let mut observation = task_timing::AppendObservation::default();
        let result = self
            .append_event_raw_with_observation(
                severity,
                source,
                module,
                actor,
                links,
                (payload, Some(&mut observation)),
            )
            .map_err(RequestFailure::poison_without_terminal);
        (result, observation)
    }

    pub(super) fn append_event_raw(
        &self,
        severity: EventSeverity,
        source: EventSource,
        module: OriginModule,
        actor: EventActor,
        links: EventLinksDraft,
        payload: impl Into<actingcommand_contract::EventPayloadDraft>,
    ) -> RuntimeHostResult<PersistedEvent> {
        self.append_event_raw_with_observation(
            severity,
            source,
            module,
            actor,
            links,
            (payload, None),
        )
    }

    fn append_event_raw_with_observation(
        &self,
        severity: EventSeverity,
        source: EventSource,
        module: OriginModule,
        actor: EventActor,
        links: EventLinksDraft,
        input: (
            impl Into<actingcommand_contract::EventPayloadDraft>,
            Option<&mut task_timing::AppendObservation>,
        ),
    ) -> RuntimeHostResult<PersistedEvent> {
        let (payload, mut observation) = input;
        if let Some(value) = observation.as_mut() {
            value.fact_gate.begin();
        }
        let gate = lock(&self.fact_write_gate, "append_runtime_event");
        if let Some(value) = observation.as_mut() {
            value.fact_gate.finish(gate.is_ok());
        }
        let gate = gate?;
        let event = self.append_event_under_fact_gate_with_observation(
            severity,
            source,
            module,
            actor,
            links,
            (payload, observation.as_deref_mut()),
        )?;
        if let Some(value) = observation.as_mut() {
            value.fact_sync.begin();
        }
        let synchronized = self.synchronize_fact_store_under_gate();
        if let Some(value) = observation.as_mut() {
            value.fact_sync.finish(synchronized.is_ok());
        }
        synchronized?;
        drop(gate);
        if let Some(value) = observation.as_mut() {
            value.pipeline.begin();
        }
        let pipeline = self.observe_pipeline_event(&event);
        if let Some(value) = observation.as_mut() {
            value.pipeline.finish(pipeline.is_ok());
        }
        pipeline?;
        Ok(event)
    }

    pub(super) fn append_event_under_fact_gate(
        &self,
        severity: EventSeverity,
        source: EventSource,
        module: OriginModule,
        actor: EventActor,
        links: EventLinksDraft,
        payload: impl Into<actingcommand_contract::EventPayloadDraft>,
    ) -> RuntimeHostResult<PersistedEvent> {
        self.append_event_under_fact_gate_with_observation(
            severity,
            source,
            module,
            actor,
            links,
            (payload, None),
        )
    }

    fn append_event_under_fact_gate_with_observation(
        &self,
        severity: EventSeverity,
        source: EventSource,
        module: OriginModule,
        actor: EventActor,
        links: EventLinksDraft,
        input: (
            impl Into<actingcommand_contract::EventPayloadDraft>,
            Option<&mut task_timing::AppendObservation>,
        ),
    ) -> RuntimeHostResult<PersistedEvent> {
        let (payload, mut observation) = input;
        if self.lifecycle_append_failed.load(Ordering::Acquire) {
            return Err(ledger_error("append_runtime_event"));
        }
        if let Some(value) = observation.as_mut() {
            value.draft.begin();
        }
        let draft = (|| {
            let draft =
                self.events
                    .draft(severity, source, module, actor, links.clone(), payload)?;
            self.events.sanitize(draft)
        })();
        if let Some(value) = observation.as_mut() {
            value.draft.finish(draft.is_ok());
        }
        let draft = draft?;
        if let Some(value) = observation.as_mut() {
            value.writer_response.begin();
        }
        let appended = if let Some(value) = observation.as_mut() {
            let (result, ledger_observation) = self.ledger.append_with_observation(draft);
            value.ledger = ledger_observation;
            result
        } else {
            self.ledger.append(draft)
        };
        if let Some(value) = observation.as_mut() {
            value.writer_response.finish(appended.is_ok());
        }
        let event = appended.map_err(|_| {
            self.lifecycle_append_failed.store(true, Ordering::Release);
            ledger_error("append_runtime_event")
        })?;
        if let Some(value) = observation.as_mut() {
            value.device_diagnostics.begin();
        }
        let diagnostic = self.observe_device_diagnostics_under_fact_gate(&event);
        if let Some(value) = observation.as_mut() {
            value.device_diagnostics.finish(diagnostic.is_ok());
        }
        if let Err(error) = diagnostic {
            self.lifecycle_append_failed.store(true, Ordering::Release);
            self.fatal.mark(error.clone())?;
            return Err(error);
        }
        Ok(event)
    }

    pub(super) fn synchronize_fact_store(&self) -> RuntimeHostResult<()> {
        let _gate = lock(&self.fact_write_gate, "synchronize_fact_store")?;
        self.synchronize_fact_store_under_gate()
    }

    pub(super) fn synchronize_fact_store_under_gate(&self) -> RuntimeHostResult<()> {
        let result: RuntimeHostResult<()> = (|| {
            let mut facts = lock(&self.facts, "synchronize_fact_store")?;
            facts.synchronize(&self.ledger)?;
            for invalidation in facts.pending_invalidations() {
                if self.lifecycle_append_failed.load(Ordering::Acquire) {
                    return Err(ledger_error("append_fact_transaction"));
                }
                let work = facts.prepare_invalidation(&self.ledger, &invalidation)?;
                let links = self.events.system_links()?;
                let draft = self.events.draft(
                    EventSeverity::Info,
                    EventSource::Runtime,
                    OriginModule::FactStore,
                    EventActor::Runtime,
                    links.clone(),
                    FactPayloadDraft::invalidated(invalidation.data.clone(), AuditInput::new()),
                )?;
                let draft = self.events.sanitize(draft)?;
                let persisted = self
                    .ledger
                    .append_transaction(draft, Box::new(work))
                    .map_err(|error| {
                        let error = crate::fact_store::fact_transaction_error(error);
                        if error.is_fatal() {
                            self.lifecycle_append_failed.store(true, Ordering::Release);
                        }
                        error
                    })?;
                facts
                    .acknowledge_generated_invalidation(&invalidation.data, persisted.sequence())
                    .and_then(|()| self.observe_device_diagnostics_under_fact_gate(&persisted))
                    .inspect_err(|error| {
                        self.lifecycle_append_failed.store(true, Ordering::Release);
                        let _ = error
                            .diagnostics()
                            .recorded_event()
                            .set(*persisted.event_id());
                    })?;
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
}
