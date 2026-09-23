// SPDX-License-Identifier: AGPL-3.0-only

//! Host wiring for the Runtime's own fact store (Workflow #313).
//!
//! Every change is appended to the `GlobalLedger` before it enters the
//! memory-only store, under the same write gate as the instance fact store.
//! Nothing here reads a file: durability is the per-record event, and the
//! periodic snapshot only shortens replay.

use super::*;
use actingcommand_contract::{
    CONFIG_POLICY_INSTANCE_IDENTITY_KEY, CONFIG_POLICY_INSTANCE_KEY,
    CONFIG_POLICY_INSTANCE_SEEDED_KEY, FactValue, MAX_RUNTIME_FACT_SNAPSHOT_BYTES,
};
use actingcommand_policy::InstanceSnapshot;

/// Key families that stop describing the device once a new owner epoch starts.
const TAKEOVER_INVALIDATED_FAMILIES: [&str; 3] = ["device.", "backend.", "application."];

/// Progress belongs to the Host projection, not to the scheduler's pure store.
#[derive(Clone)]
pub(super) struct RuntimeFactState {
    pub(super) store: RuntimeFactStore,
    applied_sequence: u64,
}

impl RuntimeFactState {
    pub(super) fn synchronize_to(
        &mut self,
        ledger: &GlobalLedger,
        position: u64,
    ) -> RuntimeHostResult<bool> {
        if position < self.applied_sequence {
            return Err(runtime_fact_replay_failed(
                position,
                "projection cursor is ahead of cut",
            ));
        }
        if position == self.applied_sequence {
            return Ok(false);
        }
        let from = self
            .applied_sequence
            .checked_add(1)
            .ok_or_else(|| runtime_fact_replay_failed(position, "ledger sequence overflow"))?;
        let events = ledger
            .query(EventQuery {
                from_sequence: Some(from),
                to_sequence: Some(position),
                ..EventQuery::default()
            })
            .map_err(|_| ledger_error("synchronize_runtime_facts"))?;
        let mut changed = false;
        for event in events {
            let sequence = event.sequence();
            if self.applied_sequence.checked_add(1) != Some(sequence) {
                return Err(runtime_fact_replay_failed(
                    sequence,
                    "projection prefix is unavailable",
                ));
            }
            match event.payload() {
                EventPayload::Runtime(RuntimePayload::FactRecorded(payload)) => {
                    validate_policy_seed_record(&self.store, payload.record()).map_err(
                        |error| runtime_fact_replay_failed(sequence, &error.to_string()),
                    )?;
                    changed |= self
                        .store
                        .record(payload.record().clone())
                        .map_err(|error| {
                            runtime_fact_desync(&error, "apply_runtime_fact_record")
                        })?
                        != RuntimeFactChange::Unchanged;
                }
                EventPayload::Runtime(RuntimePayload::FactInvalidated(payload)) => {
                    let invalidation = payload.invalidation();
                    if invalidation.key == CONFIG_POLICY_INSTANCE_SEEDED_KEY {
                        return Err(runtime_fact_replay_failed(
                            sequence,
                            "policy seed occurrence cannot be invalidated",
                        ));
                    }
                    self.store
                        .invalidate(
                            &invalidation.scope,
                            &invalidation.key,
                            invalidation.reason,
                            invalidation.at_unix_ms,
                        )
                        .map_err(|error| {
                            runtime_fact_desync(&error, "apply_runtime_fact_invalidation")
                        })?;
                    changed = true;
                }
                EventPayload::Runtime(RuntimePayload::FactSnapshot(payload)) => {
                    let snapshot = payload.snapshot();
                    snapshot
                        .validate()
                        .map_err(|error| runtime_fact_replay_failed(sequence, error.code()))?;
                    if snapshot.ledger_position > self.applied_sequence
                        || !self.store.records().eq(snapshot.records.iter())
                    {
                        return Err(runtime_fact_replay_failed(
                            sequence,
                            "snapshot does not cover the applied state",
                        ));
                    }
                }
                _ if event.origin().module() == OriginModule::RuntimeFacts => {
                    return Err(runtime_fact_replay_failed(
                        sequence,
                        "unexpected event under origin module runtime-facts",
                    ));
                }
                _ => {}
            }
            self.applied_sequence = sequence;
        }
        if self.applied_sequence != position {
            return Err(runtime_fact_replay_failed(
                position,
                "projection prefix is incomplete",
            ));
        }
        Ok(changed)
    }
}

impl HostShared {
    /// Runs at the original startup/configuration boundary before worker admission.
    /// A durable marker precedes the data: partial startup never becomes a config fallback.
    pub(super) fn seed_policy_instance_facts(&self) -> RuntimeHostResult<()> {
        let _gate = lock(&self.fact_write_gate, "seed_policy_instance_facts")?;
        let inputs = lock(&self.policy_inputs, "seed_policy_instance_facts")?.clone();
        let Some(inputs) = inputs else {
            return Ok(());
        };
        self.synchronize_fact_inputs_under_gate()?;
        let registered = lock(&self.registered_instances, "bind_policy_seed")?
            .values()
            .map(|instance| (instance.instance_alias.clone(), instance.instance_id))
            .collect::<BTreeMap<_, _>>();
        for instance in &inputs.facts().instances {
            // Only registered instances have a legal fact scope. The original
            // policy authority check still refuses any unbound configured alias.
            let Some(instance_id) = registered.get(&instance.instance_id) else {
                continue;
            };
            let scope = RuntimeFactScope::Instance {
                instance_id: *instance_id,
            };
            if lock(&self.runtime_facts, "read_policy_seed")?
                .store
                .get(&scope, CONFIG_POLICY_INSTANCE_SEEDED_KEY)
                .is_some()
            {
                continue;
            }
            let now = self.clock.sample()?.unix_ms;
            self.record_runtime_fact_under_gate(RuntimeFactRecord {
                scope: scope.clone(),
                key: CONFIG_POLICY_INSTANCE_SEEDED_KEY.into(),
                value: FactValue::Boolean(true),
                observed_at_unix_ms: now,
                source: OriginModule::Runtime,
                ttl_ms: None,
            })?;
            let mut identity = instance.clone();
            identity.available = false;
            identity.unavailable_reason = None;
            identity.capability_operation_ids.clear();
            identity.preferred_task_ids.clear();
            for (key, value) in [
                (CONFIG_POLICY_INSTANCE_IDENTITY_KEY, identity),
                (CONFIG_POLICY_INSTANCE_KEY, instance.clone()),
            ] {
                if lock(&self.runtime_facts, "read_policy_seed")?
                    .store
                    .get(&scope, key)
                    .is_some()
                {
                    continue;
                }
                self.record_runtime_fact_under_gate(policy_instance_record(
                    scope.clone(),
                    key,
                    &value,
                    now,
                )?)?;
            }
        }
        Ok(())
    }

    pub(super) fn policy_instance_scope(&self, alias: &str) -> RuntimeHostResult<RuntimeFactScope> {
        lock(&self.registered_instances, "bind_policy_instance_fact")?
            .values()
            .find(|instance| instance.instance_alias == alias)
            .map(|instance| RuntimeFactScope::Instance {
                instance_id: instance.instance_id,
            })
            .ok_or_else(|| {
                policy_admission_request(
                    "policy_instance_metadata_untrusted",
                    "bind_policy_instance_fact",
                )
            })
    }

    pub(super) fn project_program_instances(
        &self,
        configured: &[InstanceSnapshot],
        store: &RuntimeFactStore,
        now_unix_ms: u64,
    ) -> RuntimeHostResult<(Vec<InstanceSnapshot>, Vec<RuntimeFactRecord>)> {
        let mut instances = Vec::with_capacity(configured.len());
        let mut revisions = Vec::new();
        // Configuration supplies only the expected registered aliases. Every
        // projected field, including fallback identity, comes from committed facts.
        for configured in configured {
            let scope = self.policy_instance_scope(&configured.instance_id)?;
            if !store
                .get(&scope, CONFIG_POLICY_INSTANCE_SEEDED_KEY)
                .is_some_and(valid_policy_seed)
            {
                return Err(policy_instance_unavailable(
                    &configured.instance_id,
                    CONFIG_POLICY_INSTANCE_SEEDED_KEY,
                    "missing or invalid seed occurrence",
                ));
            }
            let identity_record = store
                .get(&scope, CONFIG_POLICY_INSTANCE_IDENTITY_KEY)
                .ok_or_else(|| {
                    policy_instance_unavailable(
                        &configured.instance_id,
                        CONFIG_POLICY_INSTANCE_IDENTITY_KEY,
                        "missing",
                    )
                })?;
            let identity =
                decode_policy_instance(identity_record, now_unix_ms).map_err(|cause| {
                    policy_instance_unavailable(
                        &configured.instance_id,
                        CONFIG_POLICY_INSTANCE_IDENTITY_KEY,
                        cause,
                    )
                })?;
            if identity.instance_id != configured.instance_id {
                return Err(policy_instance_unavailable(
                    &configured.instance_id,
                    CONFIG_POLICY_INSTANCE_IDENTITY_KEY,
                    "alias mismatch",
                ));
            }
            revisions.push(identity_record.clone());
            let record = store.get(&scope, CONFIG_POLICY_INSTANCE_KEY);
            let decoded = record
                .ok_or("missing")
                .and_then(|record| decode_policy_instance(record, now_unix_ms))
                .and_then(|instance| {
                    if instance.instance_id == identity.instance_id
                        && instance.server_id == identity.server_id
                        && instance.game_id == identity.game_id
                        && instance.host_id == identity.host_id
                    {
                        Ok(instance)
                    } else {
                        Err("identity mismatch")
                    }
                });
            let instance = match decoded {
                Ok(instance) => instance,
                Err(cause) => {
                    let mut unavailable = identity;
                    unavailable.available = false;
                    unavailable.capability_operation_ids.clear();
                    unavailable.preferred_task_ids.clear();
                    unavailable.unavailable_reason = Some(format!(
                        "program instance fact {CONFIG_POLICY_INSTANCE_KEY}: {cause}"
                    ));
                    unavailable
                }
            };
            if let Some(record) = record {
                revisions.push(record.clone());
            }
            instances.push(instance);
        }
        revisions.sort_by(|left, right| (&left.scope, &left.key).cmp(&(&right.scope, &right.key)));
        Ok((instances, revisions))
    }

    pub(super) fn program_instances_at(
        &self,
        configured: &[InstanceSnapshot],
        position: u64,
        current_position: u64,
        historical: bool,
    ) -> RuntimeHostResult<(Vec<InstanceSnapshot>, Vec<RuntimeFactRecord>)> {
        let now = if historical {
            self.ledger
                .query(EventQuery {
                    from_sequence: Some(position),
                    to_sequence: Some(position),
                    ..EventQuery::default()
                })
                .map_err(|_| ledger_error("read_policy_fact_cut"))?
                .first()
                .filter(|event| event.sequence() == position)
                .map(|event| event.timestamp_unix_ms())
                .ok_or_else(|| {
                    policy_admission_request(
                        "policy_input_position_unavailable",
                        "read_policy_fact_cut",
                    )
                })?
        } else {
            self.clock.sample()?.unix_ms
        };
        if position == current_position {
            let state = lock(&self.runtime_facts, "project_program_instances")?;
            if state.applied_sequence != position {
                return Err(ledger_error("runtime_fact_projection_position_mismatch"));
            }
            self.project_program_instances(configured, &state.store, now)
        } else {
            let state = runtime_facts_at(&self.ledger, position)?;
            self.project_program_instances(configured, &state.store, now)
        }
    }

    pub(super) fn synchronize_runtime_facts_to_under_gate(
        &self,
        position: u64,
    ) -> RuntimeHostResult<()> {
        let changed = lock(&self.runtime_facts, "synchronize_runtime_facts")?
            .synchronize_to(&self.ledger, position)?;
        if changed {
            self.runtime_facts_dirty.store(true, Ordering::Release);
        }
        Ok(())
    }

    /// Records the in-memory runtime configuration manifest as its two
    /// `config.*` facts, ledger-first through [`Self::record_runtime_fact`].
    /// The clock is sampled once so both records share one observation time;
    /// on a restart that time is newer than the replayed records, so they are
    /// replaced rather than refused as stale. Any refusal fails startup.
    pub(super) fn record_config_manifest(
        &self,
        manifest: &RuntimeConfigManifest,
    ) -> RuntimeHostResult<()> {
        let observed_at_unix_ms = self.clock.sample()?.unix_ms;
        for record in manifest.to_fact_records(observed_at_unix_ms, OriginModule::Runtime) {
            self.record_runtime_fact(record)?;
        }
        Ok(())
    }

    /// Appends `runtime.fact_recorded` first, then accepts the record into
    /// memory. A record the store would reject is refused before the append;
    /// an identical record appends nothing. The host is the only writer; the
    /// producers are emulator instance control (`device.connected`) and the
    /// startup configuration manifest (`config.subsystems`, `config.parameters`).
    pub(super) fn record_runtime_fact(
        &self,
        record: RuntimeFactRecord,
    ) -> RuntimeHostResult<RuntimeFactChange> {
        let _gate = lock(&self.fact_write_gate, "record_runtime_fact")?;
        self.record_runtime_fact_under_gate(record)
    }

    pub(super) fn record_runtime_fact_under_gate(
        &self,
        record: RuntimeFactRecord,
    ) -> RuntimeHostResult<RuntimeFactChange> {
        let result: RuntimeHostResult<RuntimeFactChange> = (|| {
            self.synchronize_fact_inputs_under_gate()?;
            let precheck = {
                let store = lock(&self.runtime_facts, "record_runtime_fact")?;
                validate_policy_seed_record(&store.store, &record)?;
                precheck_runtime_fact(&store.store, &record)?
            };
            if let Some(unchanged) = precheck {
                return Ok(unchanged);
            }
            let change = if lock(&self.runtime_facts, "record_runtime_fact")?
                .store
                .get(&record.scope, &record.key)
                .is_some()
            {
                RuntimeFactChange::Updated
            } else {
                RuntimeFactChange::Inserted
            };
            let links = self.runtime_fact_links(&record.scope)?;
            self.append_event_under_fact_gate(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::RuntimeFacts,
                EventActor::Runtime,
                links,
                RuntimePayloadDraft::fact_recorded(record, AuditInput::new()),
            )?;
            self.synchronize_fact_store_under_gate()?;
            Ok(change)
        })();
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.fact_projection_failed.store(true, Ordering::Release);
            self.fatal.mark(error.clone())?;
        }
        result
    }

    /// Appends `runtime.fact_invalidated` first, then drops the record from
    /// memory. A key the store does not hold is refused before the append
    /// (`runtime_fact_missing`).
    pub(super) fn invalidate_runtime_fact(
        &self,
        scope: &RuntimeFactScope,
        key: &str,
        reason: RuntimeFactInvalidationReason,
    ) -> RuntimeHostResult<RuntimeFactInvalidation> {
        let _gate = lock(&self.fact_write_gate, "invalidate_runtime_fact")?;
        self.invalidate_runtime_fact_under_gate(scope, key, reason)
    }

    pub(super) fn invalidate_runtime_fact_under_gate(
        &self,
        scope: &RuntimeFactScope,
        key: &str,
        reason: RuntimeFactInvalidationReason,
    ) -> RuntimeHostResult<RuntimeFactInvalidation> {
        let result: RuntimeHostResult<RuntimeFactInvalidation> = (|| {
            self.synchronize_fact_inputs_under_gate()?;
            if key == CONFIG_POLICY_INSTANCE_SEEDED_KEY {
                return Err(policy_admission_request(
                    "policy_instance_seed_immutable",
                    "invalidate_runtime_fact",
                ));
            }
            if lock(&self.runtime_facts, "invalidate_runtime_fact")?
                .store
                .get(scope, key)
                .is_none()
            {
                return Err(runtime_fact_rejection(
                    &RuntimeFactError::Missing,
                    "invalidate_runtime_fact",
                ));
            }
            let at_unix_ms = self.clock.sample()?.unix_ms;
            let invalidation = RuntimeFactInvalidation {
                scope: scope.clone(),
                key: key.to_owned(),
                reason,
                at_unix_ms,
            };
            invalidation.validate().map_err(|error| {
                RuntimeHostError::request(
                    error.code(),
                    "invalidate_runtime_fact",
                    RuntimeErrorCode::InvalidRequest,
                )
            })?;
            let links = self.runtime_fact_links(scope)?;
            self.append_event_under_fact_gate(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::RuntimeFacts,
                EventActor::Runtime,
                links,
                RuntimePayloadDraft::fact_invalidated(invalidation.clone(), AuditInput::new()),
            )?;
            self.synchronize_fact_store_under_gate()?;
            Ok(invalidation)
        })();
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.fact_projection_failed.store(true, Ordering::Release);
            self.fatal.mark(error.clone())?;
        }
        result
    }

    /// Seals the live store at the ledger's latest sequence. Writes nothing.
    pub(super) fn runtime_fact_snapshot(&self) -> RuntimeHostResult<RuntimeFactSnapshot> {
        let _gate = lock(&self.fact_write_gate, "read_runtime_fact_snapshot")?;
        let ledger_position = self.synchronize_fact_inputs_under_gate()?;
        let taken_at_unix_ms = self.clock.sample()?.unix_ms;
        Ok(lock(&self.runtime_facts, "read_runtime_fact_snapshot")?
            .store
            .snapshot(ledger_position, taken_at_unix_ms))
    }

    /// Appends one `runtime.fact_snapshot` when the store changed since the
    /// last seal. A never-changed store appends nothing.
    pub(super) fn append_runtime_fact_snapshot_if_dirty(&self) -> RuntimeHostResult<bool> {
        let _gate = lock(&self.fact_write_gate, "append_runtime_fact_snapshot")?;
        let result: RuntimeHostResult<bool> = (|| {
            let ledger_position = self.synchronize_fact_inputs_under_gate()?;
            if !self.runtime_facts_dirty.load(Ordering::Acquire) {
                return Ok(false);
            }
            let taken_at_unix_ms = self.clock.sample()?.unix_ms;
            let snapshot = lock(&self.runtime_facts, "append_runtime_fact_snapshot")?
                .store
                .snapshot(ledger_position, taken_at_unix_ms);
            // The typed size and position codes surface here; sanitization would fold them.
            snapshot.validate().map_err(|error| {
                RuntimeHostError::fatal(
                    error.code(),
                    "append_runtime_fact_snapshot",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
            let links = self.events.system_links()?;
            self.append_event_under_fact_gate(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::RuntimeFacts,
                EventActor::Runtime,
                links,
                RuntimePayloadDraft::fact_snapshot(snapshot, AuditInput::new()),
            )?;
            self.runtime_facts_dirty.store(false, Ordering::Release);
            self.synchronize_fact_store_under_gate()?;
            Ok(true)
        })();
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.fact_projection_failed.store(true, Ordering::Release);
            self.fatal.mark(error.clone())?;
        }
        result
    }

    /// System links, plus the registered instance link for an instance scope.
    fn runtime_fact_links(&self, scope: &RuntimeFactScope) -> RuntimeHostResult<EventLinksDraft> {
        let links = self.events.system_links()?;
        match scope {
            RuntimeFactScope::Runtime => Ok(links),
            RuntimeFactScope::Instance { instance_id } => {
                if !lock(&self.registered_instances, "bind_runtime_fact_scope")?
                    .contains_key(instance_id)
                {
                    return Err(RuntimeHostError::request(
                        "runtime_fact_instance_unknown",
                        "bind_runtime_fact_scope",
                        RuntimeErrorCode::InstanceUnknown,
                    ));
                }
                Ok(links
                    .with_instance_id(self.events.issuer().issue_registered_instance(*instance_id)))
            }
        }
    }
}

/// Rebuilds the store from the newest `runtime.fact_snapshot` plus every
/// `runtime.fact_recorded` / `runtime.fact_invalidated` appended after it, in
/// ledger order. On owner takeover, device-bound instance facts are then
/// invalidated, ledger first. Returns the store and whether it is dirty.
pub(super) fn recover_runtime_fact_store(
    ledger: &GlobalLedger,
    events: &RuntimeEvents,
    takeover: bool,
    now_unix_ms: u64,
) -> RuntimeHostResult<(RuntimeFactState, bool)> {
    let position = ledger
        .latest_sequence()
        .map_err(|_| ledger_error("recover_runtime_facts"))?;
    let mut state = runtime_facts_at(ledger, position)?;
    let dirty = if takeover {
        append_takeover_invalidations(ledger, events, &mut state, now_unix_ms)?
    } else {
        false
    };
    Ok((state, dirty))
}

/// Read-only prefix projection. A checkpoint must itself exist at the requested
/// cut and cannot cover future events or omit changes before its own append.
pub(super) fn runtime_facts_at(
    ledger: &GlobalLedger,
    position: u64,
) -> RuntimeHostResult<RuntimeFactState> {
    let latest = ledger
        .latest_sequence()
        .map_err(|_| ledger_error("project_runtime_fact_history"))?;
    if position > latest {
        return Err(RuntimeHostError::request(
            "policy_input_position_unavailable",
            "project_runtime_fact_history",
            RuntimeErrorCode::RuntimeUnavailable,
        ));
    }
    let mut state = RuntimeFactState {
        store: RuntimeFactStore::new(),
        applied_sequence: 0,
    };
    if position == 0 {
        return Ok(state);
    }
    let snapshots = ledger
        .query(EventQuery {
            event_type: Some(EventType::RuntimeFactSnapshot),
            to_sequence: Some(position),
            ..EventQuery::default()
        })
        .map_err(|_| ledger_error("project_runtime_fact_history"))?;
    if let Some(event) = snapshots.last() {
        let EventPayload::Runtime(RuntimePayload::FactSnapshot(payload)) = event.payload() else {
            return Err(runtime_fact_replay_failed(
                event.sequence(),
                "invalid checkpoint payload",
            ));
        };
        let snapshot = payload.snapshot();
        snapshot
            .validate()
            .map_err(|error| runtime_fact_replay_failed(event.sequence(), error.code()))?;
        if snapshot.ledger_position >= event.sequence() {
            return Err(runtime_fact_replay_failed(
                event.sequence(),
                "checkpoint covers a future position",
            ));
        }
        if snapshot.ledger_position + 1 < event.sequence() {
            let interval = ledger
                .query(EventQuery {
                    from_sequence: Some(snapshot.ledger_position + 1),
                    to_sequence: Some(event.sequence() - 1),
                    ..EventQuery::default()
                })
                .map_err(|_| ledger_error("verify_runtime_fact_checkpoint"))?;
            let mut expected = snapshot.ledger_position + 1;
            for prior in interval {
                if prior.sequence() != expected
                    || matches!(
                        prior.payload(),
                        EventPayload::Runtime(
                            RuntimePayload::FactRecorded(_) | RuntimePayload::FactInvalidated(_)
                        )
                    )
                {
                    return Err(runtime_fact_replay_failed(
                        event.sequence(),
                        "checkpoint coverage is incomplete",
                    ));
                }
                expected += 1;
            }
            if expected != event.sequence() {
                return Err(runtime_fact_replay_failed(
                    event.sequence(),
                    "checkpoint prefix is unavailable",
                ));
            }
        }
        state
            .store
            .replay(snapshot)
            .map_err(|error| runtime_fact_replay_failed(event.sequence(), &error.to_string()))?;
        for record in state
            .store
            .records()
            .filter(|record| record.key == CONFIG_POLICY_INSTANCE_SEEDED_KEY)
        {
            validate_policy_seed_record(&state.store, record).map_err(|error| {
                runtime_fact_replay_failed(event.sequence(), &error.to_string())
            })?;
        }
        state.applied_sequence = event.sequence();
    }
    state
        .synchronize_to(ledger, position)
        .map_err(|error| runtime_fact_replay_failed(position, &error.to_string()))?;
    Ok(state)
}

/// Appends `runtime.fact_invalidated` (reason `runtime_takeover`) for every
/// `device.` / `backend.` / `application.` instance fact, applying each append. Zero
/// matching records append nothing.
fn append_takeover_invalidations(
    ledger: &GlobalLedger,
    events: &RuntimeEvents,
    state: &mut RuntimeFactState,
    at_unix_ms: u64,
) -> RuntimeHostResult<bool> {
    let mut targets: BTreeMap<InstanceId, Vec<String>> = BTreeMap::new();
    for record in state.store.records() {
        if let RuntimeFactScope::Instance { instance_id } = &record.scope
            && TAKEOVER_INVALIDATED_FAMILIES
                .iter()
                .any(|family| record.key.starts_with(family))
        {
            targets
                .entry(*instance_id)
                .or_default()
                .push(record.key.clone());
        }
    }
    if targets.is_empty() {
        return Ok(false);
    }
    for (instance_id, keys) in &targets {
        let links = events
            .system_links()?
            .with_instance_id(events.issuer().issue_registered_instance(*instance_id));
        for key in keys {
            let invalidation = RuntimeFactInvalidation {
                scope: RuntimeFactScope::Instance {
                    instance_id: *instance_id,
                },
                key: key.clone(),
                reason: RuntimeFactInvalidationReason::RuntimeTakeover,
                at_unix_ms,
            };
            let draft = events.draft(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::RuntimeFacts,
                EventActor::Runtime,
                links.clone(),
                RuntimePayloadDraft::fact_invalidated(invalidation, AuditInput::new()),
            )?;
            let draft = events.sanitize(draft)?;
            let event = ledger
                .append(draft)
                .map_err(|_| ledger_error("append_runtime_fact_invalidated"))?;
            state.synchronize_to(ledger, event.sequence())?;
        }
    }
    Ok(true)
}

pub(super) fn policy_instance_record(
    scope: RuntimeFactScope,
    key: &str,
    instance: &InstanceSnapshot,
    observed_at_unix_ms: u64,
) -> RuntimeHostResult<RuntimeFactRecord> {
    let value = serde_json::to_string(instance).map_err(|_| {
        RuntimeHostError::fatal(
            "policy_instance_fact_encode_failed",
            "seed_policy_instance_facts",
            RuntimeErrorCode::RuntimeFatal,
        )
    })?;
    if value.len() > MAX_RUNTIME_FACT_SNAPSHOT_BYTES {
        return Err(policy_admission_request(
            "policy_instance_fact_payload_too_large",
            "seed_policy_instance_facts",
        ));
    }
    Ok(RuntimeFactRecord {
        scope,
        key: key.to_owned(),
        value: FactValue::String(value),
        observed_at_unix_ms,
        source: OriginModule::Runtime,
        ttl_ms: None,
    })
}

fn decode_policy_instance(
    record: &RuntimeFactRecord,
    now_unix_ms: u64,
) -> Result<InstanceSnapshot, &'static str> {
    if record.is_expired(now_unix_ms) {
        return Err("expired");
    }
    let FactValue::String(encoded) = &record.value else {
        return Err("invalid value type");
    };
    if encoded.len() > MAX_RUNTIME_FACT_SNAPSHOT_BYTES {
        return Err("value exceeds snapshot byte bound");
    }
    let decoded: InstanceSnapshot =
        serde_json::from_str(encoded).map_err(|_| "invalid encoding or missing field")?;
    decoded
        .validate_metadata()
        .map_err(|_| "invalid instance field")?;
    // Reasons are derived from missing/invalid facts by the Host, not seeded inputs.
    if decoded.unavailable_reason.is_some() {
        return Err("stored derived reason");
    }
    Ok(decoded)
}

fn policy_instance_unavailable(alias: &str, key: &str, cause: &str) -> RuntimeHostError {
    RuntimeHostError::request(
        "policy_instance_fact_unavailable",
        "project_program_instances",
        RuntimeErrorCode::RuntimeUnavailable,
    )
    .with_native_detail(format!("instance={alias}; key={key}; cause={cause}"))
}

fn valid_policy_seed(record: &RuntimeFactRecord) -> bool {
    matches!(record.scope, RuntimeFactScope::Instance { .. })
        && record.value == FactValue::Boolean(true)
        && record.source == OriginModule::Runtime
        && record.ttl_ms.is_none()
}

fn validate_policy_seed_record(
    store: &RuntimeFactStore,
    record: &RuntimeFactRecord,
) -> RuntimeHostResult<()> {
    if record.key == CONFIG_POLICY_INSTANCE_SEEDED_KEY
        && (!valid_policy_seed(record)
            || store
                .get(&record.scope, &record.key)
                .is_some_and(|prior| prior != record))
    {
        return Err(policy_admission_request(
            "policy_instance_seed_immutable",
            "record_runtime_fact",
        ));
    }
    Ok(())
}

/// Applies the store's acceptance rules without mutating it. `Some` carries
/// the idempotent outcome for an identical record; `None` means the record
/// would be inserted or would replace an older observation.
fn precheck_runtime_fact(
    store: &RuntimeFactStore,
    record: &RuntimeFactRecord,
) -> RuntimeHostResult<Option<RuntimeFactChange>> {
    record.validate().map_err(|error| {
        runtime_fact_rejection(
            &RuntimeFactError::Invalid { code: error.code() },
            "record_runtime_fact",
        )
    })?;
    match store.get(&record.scope, &record.key) {
        Some(existing) if existing == record => Ok(Some(RuntimeFactChange::Unchanged)),
        Some(existing) if existing.observed_at_unix_ms >= record.observed_at_unix_ms => {
            Err(runtime_fact_rejection(
                &RuntimeFactError::Stale {
                    existing_observed_at_unix_ms: existing.observed_at_unix_ms,
                },
                "record_runtime_fact",
            ))
        }
        Some(_) => Ok(None),
        None if store.len() >= MAX_RUNTIME_FACTS => Err(runtime_fact_rejection(
            &RuntimeFactError::CapacityExceeded {
                limit: MAX_RUNTIME_FACTS,
            },
            "record_runtime_fact",
        )),
        None => Ok(None),
    }
}

const fn runtime_fact_code(error: &RuntimeFactError) -> &'static str {
    match error {
        RuntimeFactError::Invalid { code } => code,
        RuntimeFactError::Stale { .. } => "runtime_fact_stale",
        RuntimeFactError::CapacityExceeded { .. } => "runtime_fact_capacity_exceeded",
        RuntimeFactError::Missing => "runtime_fact_missing",
    }
}

/// A store rule refused the change before anything was appended.
fn runtime_fact_rejection(error: &RuntimeFactError, operation: &'static str) -> RuntimeHostError {
    RuntimeHostError::request(
        runtime_fact_code(error),
        operation,
        RuntimeErrorCode::InvalidRequest,
    )
    .with_native_detail(error.to_string())
}

/// The store refused a change that was already appended: memory and ledger disagree.
fn runtime_fact_desync(error: &RuntimeFactError, operation: &'static str) -> RuntimeHostError {
    RuntimeHostError::fatal(
        "runtime_fact_store_desync",
        operation,
        RuntimeErrorCode::RuntimeFatal,
    )
    .with_native_detail(format!("{}: {error}", runtime_fact_code(error)))
}

/// Our own ledger holds a runtime-facts event the store refuses to replay.
fn runtime_fact_replay_failed(sequence: u64, detail: &str) -> RuntimeHostError {
    RuntimeHostError::fatal(
        "runtime_fact_replay_failed",
        "recover_runtime_facts",
        RuntimeErrorCode::RuntimeFatal,
    )
    .with_native_detail(format!("sequence {sequence}: {detail}"))
}
