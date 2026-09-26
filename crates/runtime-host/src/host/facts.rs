// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::fact_store::{priority_offset_milli, valid_priority_offset_task_id};
use actingcommand_contract::priority_offset_task_id;

/// `source_detector` of the three configuration-seeded policy instance facts.
const POLICY_INSTANCE_SEED_DETECTOR: &str = "runtime.policy-configuration";
const POLICY_INSTANCE_SEED_SCHEMA: &str = "fact.v1";
/// `source_detector` and `source_snapshot_id` prefix of the `session.instance.available = false`
/// records a failed backend self-check publishes (Workflow #317 sc2).
const BACKEND_SELFCHECK_AVAILABILITY_DETECTOR: &str = "runtime.backend-selfcheck";
pub(super) const BACKEND_SELFCHECK_AVAILABILITY_SNAPSHOT_PREFIX: &str = "backend_selfcheck:";

/// One `record_list` row per identifier, each holding the single string field `field`.
fn policy_instance_string_list_value(field: &str, identifiers: &[String]) -> ContractFactValue {
    ContractFactValue::RecordList(
        identifiers
            .iter()
            .map(|identifier| {
                BTreeMap::from([(field.to_owned(), FactScalar::String(identifier.clone()))])
            })
            .collect(),
    )
}

/// The stable hash a seed carries as `source_snapshot_id` and `resource_bundle_hash`: SHA-256
/// over the canonical JSON of (instance alias, key, configured value), as lowercase hex.
fn policy_instance_seed_digest(
    instance_id: &str,
    key: &str,
    value: &ContractFactValue,
) -> RuntimeHostResult<String> {
    let bytes = serde_json::to_vec(&(instance_id, key, value)).map_err(|_| {
        RuntimeHostError::fatal(
            "policy_instance_seed_encode_failed",
            "seed_policy_instance_facts",
            RuntimeErrorCode::RuntimeFatal,
        )
    })?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

impl HostShared {
    pub(super) fn publish_fact(&self, record: FactRecord) -> RuntimeHostResult<EventId> {
        self.publish_facts(
            actingcommand_contract::FactObservation {
                records: vec![record],
            },
            None,
        )
    }

    /// Seeds the policy instance facts from the configured policy inputs (Workflow #313
    /// item 4): for every configured instance, `session.instance.available`,
    /// `session.instance.capabilities` and `session.instance.preferred_tasks`, each with
    /// scope = that instance. A record's `source_snapshot_id` is a stable hash of the
    /// instance alias, the key and the configured value: a key whose stored record already
    /// carries that id is already published and appends nothing (its content is verified
    /// against the configured value); otherwise the seed is a new observation at the host
    /// clock. Any refusal is returned to the caller, which fails startup.
    pub(super) fn seed_policy_instance_facts(&self) -> RuntimeHostResult<()> {
        let Some(inputs) = lock(&self.policy_inputs, "seed_policy_instance_facts")?.clone() else {
            return Ok(());
        };
        for instance in inputs.instance_seeds() {
            let scope = FactScope::Instance {
                instance_id: instance.instance_id.clone(),
            };
            let seeds = [
                (
                    POLICY_INSTANCE_AVAILABLE_KEY,
                    ContractFactValue::Boolean(instance.available),
                ),
                (
                    POLICY_INSTANCE_CAPABILITIES_KEY,
                    policy_instance_string_list_value(
                        POLICY_INSTANCE_OPERATION_FIELD,
                        &instance.capability_operation_ids,
                    ),
                ),
                (
                    POLICY_INSTANCE_PREFERRED_TASKS_KEY,
                    policy_instance_string_list_value(
                        POLICY_INSTANCE_TASK_FIELD,
                        &instance.preferred_task_ids,
                    ),
                ),
            ];
            for (key, value) in seeds {
                let digest = policy_instance_seed_digest(&instance.instance_id, key, &value)?;
                let source_snapshot_id = format!("snapshot:policy-config:{digest}");
                let content = FactContent::Inline { value };
                let already_published = {
                    let _gate = lock(&self.fact_write_gate, "seed_policy_instance_facts")?;
                    let stored = lock(&self.facts, "seed_policy_instance_facts")?;
                    match stored.active_record(&scope, key) {
                        Some(existing) if existing.source_snapshot_id == source_snapshot_id => {
                            if existing.content != content {
                                return Err(RuntimeHostError::fatal(
                                    "policy_instance_seed_identity_conflict",
                                    "seed_policy_instance_facts",
                                    RuntimeErrorCode::RuntimeFatal,
                                ));
                            }
                            true
                        }
                        _ => false,
                    }
                };
                if already_published {
                    continue;
                }
                self.publish_fact(FactRecord {
                    scope: scope.clone(),
                    key: key.to_owned(),
                    content,
                    observed_at_unix_ms: self.clock.sample()?.unix_ms,
                    expires_at_unix_ms: None,
                    ttl_policy: None,
                    confidence_milli: 1_000,
                    source_detector: POLICY_INSTANCE_SEED_DETECTOR.to_owned(),
                    source_snapshot_id,
                    schema_version: POLICY_INSTANCE_SEED_SCHEMA.to_owned(),
                    resource_bundle_hash: digest,
                    invalidate_on: Vec::new(),
                })?;
            }
        }
        Ok(())
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
            // A priority offset (Workflow #308 slice 4a-2) names a task of the scheduling
            // identifier charset, holds an inline integer within ±1 000 000 milli, and is never
            // invalidated by events (its lifetime is the publisher's TTL).
            for record in &observation.records {
                if let Some(task_id) = priority_offset_task_id(&record.key)
                    && (!valid_priority_offset_task_id(task_id)
                        || priority_offset_milli(record).is_none()
                        || !record.invalidate_on.is_empty())
                {
                    return Err(RuntimeHostError::request(
                        "priority_offset_invalid",
                        "publish_facts",
                        RuntimeErrorCode::InvalidRequest,
                    ));
                }
            }
            // An offset-only observation keeps its request's origin on the `fact.published`
            // event, so the ledger shows who set the offset; every other publication stays a
            // Runtime fact-store event.
            let (event_source, event_actor) = match source_request {
                Some(request)
                    if !observation.records.is_empty()
                        && observation
                            .records
                            .iter()
                            .all(|record| priority_offset_task_id(&record.key).is_some()) =>
                {
                    (request.source(), request.actor())
                }
                _ => (EventSource::Runtime, EventActor::Runtime),
            };
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
                event_source,
                OriginModule::FactStore,
                event_actor,
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

    /// Workflow #317 sc2: publishes `session.instance.available = false` for one configured
    /// policy instance because its `<entry>` self-check of session `<generation>` failed, through
    /// the seed's publisher ([`Self::publish_fact`]) and scope. The record differs from the seed
    /// only in `source_detector` `runtime.backend-selfcheck` and `source_snapshot_id`
    /// `backend_selfcheck:<entry>:<generation>`; `resource_bundle_hash` is the seed digest of the
    /// value `false`.
    pub(super) fn publish_backend_selfcheck_unavailable(
        &self,
        instance_alias: &str,
        entry: &str,
        generation: u64,
    ) -> RuntimeHostResult<()> {
        self.publish_policy_instance_availability(
            instance_alias,
            false,
            BACKEND_SELFCHECK_AVAILABILITY_DETECTOR,
            |_| format!("{BACKEND_SELFCHECK_AVAILABILITY_SNAPSHOT_PREFIX}{entry}:{generation}"),
        )
    }

    /// Workflow #317 sc2: republishes the configured seed of `session.instance.available`
    /// (`available`, with the seed's own detector, snapshot id and digest) as a new observation
    /// at the host clock, through [`Self::publish_fact`].
    pub(super) fn restore_policy_instance_availability(
        &self,
        instance_alias: &str,
        available: bool,
    ) -> RuntimeHostResult<()> {
        self.publish_policy_instance_availability(
            instance_alias,
            available,
            POLICY_INSTANCE_SEED_DETECTOR,
            |digest| format!("snapshot:policy-config:{digest}"),
        )
    }

    fn publish_policy_instance_availability(
        &self,
        instance_alias: &str,
        available: bool,
        source_detector: &str,
        source_snapshot_id: impl FnOnce(&str) -> String,
    ) -> RuntimeHostResult<()> {
        let value = ContractFactValue::Boolean(available);
        let digest =
            policy_instance_seed_digest(instance_alias, POLICY_INSTANCE_AVAILABLE_KEY, &value)?;
        self.publish_fact(FactRecord {
            scope: FactScope::Instance {
                instance_id: instance_alias.to_owned(),
            },
            key: POLICY_INSTANCE_AVAILABLE_KEY.to_owned(),
            content: FactContent::Inline { value },
            observed_at_unix_ms: self.clock.sample()?.unix_ms,
            expires_at_unix_ms: None,
            ttl_policy: None,
            confidence_milli: 1_000,
            source_detector: source_detector.to_owned(),
            source_snapshot_id: source_snapshot_id(&digest),
            schema_version: POLICY_INSTANCE_SEED_SCHEMA.to_owned(),
            resource_bundle_hash: digest,
            invalidate_on: Vec::new(),
        })
        .map(|_| ())
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
