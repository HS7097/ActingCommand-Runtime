// SPDX-License-Identifier: AGPL-3.0-only

//! Host wiring for the Runtime's own fact store (Workflow #313).
//!
//! Every change is appended to the `GlobalLedger` before it enters the
//! memory-only store, under the same write gate as the instance fact store.
//! Nothing here reads a file: durability is the per-record event, and the
//! periodic snapshot only shortens replay.

use super::*;

/// Key families that stop describing the device once a new owner epoch starts.
const TAKEOVER_INVALIDATED_FAMILIES: [&str; 3] = ["device.", "backend.", "application."];

impl HostShared {
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
        let result: RuntimeHostResult<RuntimeFactChange> = (|| {
            let _gate = lock(&self.fact_write_gate, "record_runtime_fact")?;
            let precheck = {
                let store = lock(&self.runtime_facts, "record_runtime_fact")?;
                precheck_runtime_fact(&store, &record)?
            };
            if let Some(unchanged) = precheck {
                return Ok(unchanged);
            }
            let links = self.runtime_fact_links(&record.scope)?;
            self.append_event_under_fact_gate(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::RuntimeFacts,
                EventActor::Runtime,
                links,
                RuntimePayloadDraft::fact_recorded(record.clone(), AuditInput::new()),
            )?;
            let change = lock(&self.runtime_facts, "record_runtime_fact")?
                .record(record)
                .map_err(|error| runtime_fact_desync(&error, "record_runtime_fact"))?;
            self.runtime_facts_dirty.store(true, Ordering::Release);
            self.synchronize_fact_store_under_gate()?;
            Ok(change)
        })();
        if let Err(error) = &result
            && error.is_fatal()
        {
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
        let result: RuntimeHostResult<RuntimeFactInvalidation> = (|| {
            let _gate = lock(&self.fact_write_gate, "invalidate_runtime_fact")?;
            if lock(&self.runtime_facts, "invalidate_runtime_fact")?
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
                RuntimePayloadDraft::fact_invalidated(invalidation, AuditInput::new()),
            )?;
            let invalidation = lock(&self.runtime_facts, "invalidate_runtime_fact")?
                .invalidate(scope, key, reason, at_unix_ms)
                .map_err(|error| runtime_fact_desync(&error, "invalidate_runtime_fact"))?;
            self.runtime_facts_dirty.store(true, Ordering::Release);
            self.synchronize_fact_store_under_gate()?;
            Ok(invalidation)
        })();
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.fatal.mark(error.clone())?;
        }
        result
    }

    /// Seals the live store at the ledger's latest sequence. Writes nothing.
    pub(super) fn runtime_fact_snapshot(&self) -> RuntimeHostResult<RuntimeFactSnapshot> {
        let _gate = lock(&self.fact_write_gate, "read_runtime_fact_snapshot")?;
        let ledger_position = self
            .ledger
            .latest_sequence()
            .map_err(|_| ledger_error("read_runtime_fact_position"))?;
        let taken_at_unix_ms = self.clock.sample()?.unix_ms;
        Ok(lock(&self.runtime_facts, "read_runtime_fact_snapshot")?
            .snapshot(ledger_position, taken_at_unix_ms))
    }

    /// Appends one `runtime.fact_snapshot` when the store changed since the
    /// last seal. A never-changed store appends nothing.
    pub(super) fn append_runtime_fact_snapshot_if_dirty(&self) -> RuntimeHostResult<bool> {
        let result: RuntimeHostResult<bool> = (|| {
            let _gate = lock(&self.fact_write_gate, "append_runtime_fact_snapshot")?;
            if !self.runtime_facts_dirty.load(Ordering::Acquire) {
                return Ok(false);
            }
            let ledger_position = self
                .ledger
                .latest_sequence()
                .map_err(|_| ledger_error("read_runtime_fact_position"))?;
            let taken_at_unix_ms = self.clock.sample()?.unix_ms;
            let snapshot = lock(&self.runtime_facts, "append_runtime_fact_snapshot")?
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
) -> RuntimeHostResult<(RuntimeFactStore, bool)> {
    let mut store = RuntimeFactStore::new();
    let snapshots = ledger
        .query(EventQuery {
            event_type: Some(EventType::RuntimeFactSnapshot),
            ..EventQuery::default()
        })
        .map_err(|_| ledger_error("recover_runtime_facts"))?;
    let mut from_sequence = 1;
    if let Some(latest) = snapshots.last() {
        let EventPayload::Runtime(RuntimePayload::FactSnapshot(payload)) = latest.payload() else {
            return Err(runtime_fact_replay_failed(
                latest.sequence(),
                "payload is not runtime.fact_snapshot",
            ));
        };
        store
            .replay(payload.snapshot())
            .map_err(|error| runtime_fact_replay_failed(latest.sequence(), &error.to_string()))?;
        from_sequence = latest.sequence().checked_add(1).ok_or_else(|| {
            runtime_fact_replay_failed(latest.sequence(), "ledger sequence overflow")
        })?;
    }
    let appended = ledger
        .query(EventQuery {
            from_sequence: Some(from_sequence),
            origin_module: Some(OriginModule::RuntimeFacts),
            ..EventQuery::default()
        })
        .map_err(|_| ledger_error("recover_runtime_facts"))?;
    for event in &appended {
        let sequence = event.sequence();
        match event.payload() {
            EventPayload::Runtime(RuntimePayload::FactRecorded(payload)) => {
                store
                    .record(payload.record().clone())
                    .map_err(|error| runtime_fact_replay_failed(sequence, &error.to_string()))?;
            }
            EventPayload::Runtime(RuntimePayload::FactInvalidated(payload)) => {
                let invalidation = payload.invalidation();
                store
                    .invalidate(
                        &invalidation.scope,
                        &invalidation.key,
                        invalidation.reason,
                        invalidation.at_unix_ms,
                    )
                    .map_err(|error| runtime_fact_replay_failed(sequence, &error.to_string()))?;
            }
            _ => {
                return Err(runtime_fact_replay_failed(
                    sequence,
                    "unexpected event under origin module runtime-facts",
                ));
            }
        }
    }
    let dirty = if takeover {
        append_takeover_invalidations(ledger, events, &mut store, now_unix_ms)?
    } else {
        false
    };
    Ok((store, dirty))
}

/// Appends `runtime.fact_invalidated` (reason `runtime_takeover`) for every
/// `device.` / `backend.` instance fact, then drops them per instance. Zero
/// matching records append nothing.
fn append_takeover_invalidations(
    ledger: &GlobalLedger,
    events: &RuntimeEvents,
    store: &mut RuntimeFactStore,
    at_unix_ms: u64,
) -> RuntimeHostResult<bool> {
    let mut targets: BTreeMap<InstanceId, Vec<String>> = BTreeMap::new();
    for record in store.records() {
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
            ledger
                .append(draft)
                .map_err(|_| ledger_error("append_runtime_fact_invalidated"))?;
        }
        let dropped = store.invalidate_instance(
            *instance_id,
            &TAKEOVER_INVALIDATED_FAMILIES,
            RuntimeFactInvalidationReason::RuntimeTakeover,
            at_unix_ms,
        );
        if dropped.iter().map(|entry| &entry.key).ne(keys.iter()) {
            return Err(RuntimeHostError::fatal(
                "runtime_fact_store_desync",
                "invalidate_runtime_facts_on_takeover",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
    }
    Ok(true)
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
