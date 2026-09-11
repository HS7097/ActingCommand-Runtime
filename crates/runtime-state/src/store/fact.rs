// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::{
    EventActor, EventPayload, EventSource, EventType, FactInvalidationEventData, FactPayload,
    FactScope, OriginModule,
};
use actingcommand_ledger::PersistedEvent;
use actingcommand_runtime_database::RuntimeTransaction;

pub const FACT_TOMBSTONE_NAMESPACE: &str = "fact.tombstone.v1";

pub enum FactStateObservation {
    Applied,
    Unchanged,
    Unknown,
}

pub struct PreparedFactProjection {
    store: Arc<RuntimeStateStore>,
    data: FactInvalidationEventData,
    published: PersistedEvent,
    trigger: PersistedEvent,
    key: String,
    payload: Vec<u8>,
    baseline: Option<ProjectionEntry>,
    applied: std::sync::Mutex<Option<PersistedEvent>>,
}

impl PreparedFactProjection {
    pub fn apply(
        &self,
        transaction: &RuntimeTransaction<'_, '_>,
        event: &PersistedEvent,
    ) -> RuntimeStateResult<()> {
        self.require_owner(transaction)?;
        verify_sources(&self.data, &self.published, &self.trigger, event)?;
        for source in [&self.published, &self.trigger, event] {
            verify_original(&self.store.database, transaction, source)?;
        }
        let current = self.read_current(transaction)?;
        if let Some(entry) = &current
            && entry.ledger_sequence() == event.sequence()
        {
            require_entry(entry, event)?;
            *self
                .applied
                .lock()
                .map_err(|_| fact_error("fact_projection_work_poisoned"))? = Some(event.clone());
            return Ok(());
        }
        if current != self.baseline {
            return Err(fact_error("fact_tombstone_projection_changed"));
        }
        *self
            .applied
            .lock()
            .map_err(|_| fact_error("fact_projection_work_poisoned"))? = Some(event.clone());
        let entry = self.store.write_projection_in_transaction(
            transaction.sql(),
            FACT_TOMBSTONE_NAMESPACE,
            &self.key,
            event.sequence(),
            &self.payload,
            &sha256(&self.payload),
        )?;
        require_entry(&entry, event)
    }

    pub fn observe(
        &self,
        transaction: &RuntimeTransaction<'_, '_>,
    ) -> RuntimeStateResult<FactStateObservation> {
        self.require_owner(transaction)?;
        for source in [&self.published, &self.trigger] {
            verify_original(&self.store.database, transaction, source)?;
        }
        let current = self.read_current(transaction)?;
        let applied = self
            .applied
            .lock()
            .map_err(|_| fact_error("fact_projection_work_poisoned"))?;
        if let (Some(entry), Some(event)) = (&current, applied.as_ref())
            && entry.ledger_sequence() == event.sequence()
            && entry.payload() == self.payload
        {
            require_entry(entry, event)?;
            verify_original(&self.store.database, transaction, event)?;
            return Ok(FactStateObservation::Applied);
        }
        Ok(if current == self.baseline {
            FactStateObservation::Unchanged
        } else {
            FactStateObservation::Unknown
        })
    }

    fn require_owner(&self, transaction: &RuntimeTransaction<'_, '_>) -> RuntimeStateResult<()> {
        if !transaction.belongs_to(&self.store.database) {
            return Err(fact_error("state_transaction_owner_mismatch"));
        }
        Ok(())
    }

    fn read_current(
        &self,
        transaction: &RuntimeTransaction<'_, '_>,
    ) -> RuntimeStateResult<Option<ProjectionEntry>> {
        query_projection_entry(transaction.sql(), FACT_TOMBSTONE_NAMESPACE, &self.key)?
            .map(|row| {
                self.store
                    .validate_projection_row(row, "apply_fact_projection")
            })
            .transpose()
    }
}

impl RuntimeStateStore {
    /// Sources and State baseline are captured before entering the Ledger writer.
    pub fn prepare_fact_projection(
        self: &Arc<Self>,
        data: &FactInvalidationEventData,
        published: PersistedEvent,
        trigger: PersistedEvent,
    ) -> RuntimeStateResult<PreparedFactProjection> {
        let key = fact_tombstone_key(&data.scope, &data.key, &data.source_snapshot_id)?;
        let payload =
            serde_json::to_vec(data).map_err(|_| fact_error("fact_projection_encode_failed"))?;
        validate_projection_input(FACT_TOMBSTONE_NAMESPACE, &key, 1, &payload)?;
        let baseline = self.read_projection_entry(FACT_TOMBSTONE_NAMESPACE, &key)?;
        if baseline.is_some() {
            return Err(fact_error("fact_tombstone_projection_source_unproven"));
        }
        Ok(PreparedFactProjection {
            store: Arc::clone(self),
            data: data.clone(),
            published,
            trigger,
            key,
            payload,
            baseline,
            applied: std::sync::Mutex::new(None),
        })
    }

    /// Checks the original row again when a permanent tombstone is used for admission.
    pub fn verify_fact_projection_entry(
        &self,
        entry: &ProjectionEntry,
        event: &PersistedEvent,
    ) -> RuntimeStateResult<()> {
        require_entry(entry, event)?;
        let mut connection = self.connection("verify_fact_projection")?;
        let transaction = connection
            .transaction()
            .map_err(|error| fact_sql_error("begin_fact_verification", &error))?;
        let scope = self.database.borrow_transaction(&transaction);
        verify_original(&self.database, &scope, event)?;
        let current =
            query_projection_entry(&transaction, FACT_TOMBSTONE_NAMESPACE, entry.entry_key())?
                .map(|row| self.validate_projection_row(row, "verify_fact_projection"))
                .transpose()?;
        if current.as_ref() != Some(entry) {
            return Err(fact_error("fact_tombstone_projection_changed"));
        }
        Ok(())
    }

    /// Reconciles complete, semantically replayed history without creating another fact.
    pub fn recover_fact_projections(&self, events: &[PersistedEvent]) -> RuntimeStateResult<()> {
        let mut published = BTreeMap::<_, &PersistedEvent>::new();
        let mut originals = BTreeMap::<_, &PersistedEvent>::new();
        let mut facts = BTreeMap::new();
        let mut latest = BTreeMap::<String, &PersistedEvent>::new();
        let mut previous = 0;
        for event in events {
            if event.sequence() <= previous {
                return Err(fact_error("fact_projection_history_order_invalid"));
            }
            previous = event.sequence();
            match event.payload() {
                EventPayload::Fact(FactPayload::Published(value)) => {
                    for record in value.records() {
                        published.insert(
                            (
                                record.scope.clone(),
                                record.key.clone(),
                                record.source_snapshot_id.clone(),
                            ),
                            event,
                        );
                    }
                }
                EventPayload::Fact(FactPayload::Invalidated(value)) => {
                    let data = value.invalidation();
                    let source = published
                        .get(&(
                            data.scope.clone(),
                            data.key.clone(),
                            data.source_snapshot_id.clone(),
                        ))
                        .ok_or_else(|| fact_error("fact_invalidation_publication_missing"))?;
                    let trigger = originals
                        .get(&data.invalidated_by_event_id)
                        .ok_or_else(|| fact_error("fact_invalidation_trigger_missing"))?;
                    verify_sources(data, source, trigger, event)?;
                    let key = fact_tombstone_key(&data.scope, &data.key, &data.source_snapshot_id)?;
                    if let Some(prior) = latest.insert(key, event)
                        && !matches!(prior.payload(), EventPayload::Fact(FactPayload::Invalidated(value)) if value.invalidation() == data)
                    {
                        return Err(fact_error("fact_invalidation_identity_conflict"));
                    }
                    facts.insert(event.sequence(), (event, *source, *trigger));
                }
                _ => {}
            }
            originals.insert(*event.event_id(), event);
        }
        let mut connection = self.connection("recover_fact_projections")?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| fact_sql_error("begin_fact_recovery", &error))?;
        let work = (|| {
            let scope = self.database.borrow_transaction(&transaction);
            for (event, published, trigger) in facts.values() {
                for original in [event, published, trigger] {
                    verify_original(&self.database, &scope, original)?;
                }
            }
            let keys = {
                let mut statement = transaction.prepare("SELECT entry_key FROM projection_entries WHERE namespace=?1 ORDER BY entry_key")
                    .map_err(|error| fact_sql_error("read_fact_projection_keys", &error))?;
                statement
                    .query_map([FACT_TOMBSTONE_NAMESPACE], |row| row.get::<_, String>(0))
                    .map_err(|error| fact_sql_error("read_fact_projection_keys", &error))?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| fact_sql_error("read_fact_projection_keys", &error))?
            };
            for key in keys {
                let row = query_projection_entry(&transaction, FACT_TOMBSTONE_NAMESPACE, &key)?
                    .ok_or_else(|| fact_error("fact_tombstone_projection_missing"))?;
                let entry = self.validate_projection_row(row, "recover_fact_projections")?;
                let (event, _, _) = facts
                    .get(&entry.ledger_sequence())
                    .ok_or_else(|| fact_error("fact_tombstone_projection_source_unproven"))?;
                require_entry(&entry, event)?;
            }
            for (key, event) in latest {
                let EventPayload::Fact(FactPayload::Invalidated(value)) = event.payload() else {
                    return Err(fact_error("fact_projection_payload_mismatch"));
                };
                let payload = serde_json::to_vec(value.invalidation())
                    .map_err(|_| fact_error("fact_projection_encode_failed"))?;
                let entry = self.write_projection_in_transaction(
                    &transaction,
                    FACT_TOMBSTONE_NAMESPACE,
                    &key,
                    event.sequence(),
                    &payload,
                    &sha256(&payload),
                )?;
                require_entry(&entry, event)?;
            }
            Ok(())
        })();
        if let Err(original) = work {
            return match transaction.rollback() {
                Ok(()) => Err(original),
                Err(rollback) => Err(fact_error("fact_projection_rollback_failed")
                    .with_detail(format!("primary={original}; rollback={rollback}"))),
            };
        }
        transaction.commit().map_err(|error| {
            fact_sql_error("commit_fact_recovery", &error)
                .with_detail(format!("commit_outcome=unknown; primary={error}"))
        })?;
        Ok(())
    }
}

pub fn fact_tombstone_key(
    scope: &FactScope,
    key: &str,
    snapshot: &str,
) -> RuntimeStateResult<String> {
    let identity = serde_json::to_vec(&(scope, key, snapshot))
        .map_err(|_| fact_error("fact_tombstone_identity_encode_failed"))?;
    Ok(format!("{:x}", Sha256::digest(identity)))
}

fn require_entry(entry: &ProjectionEntry, event: &PersistedEvent) -> RuntimeStateResult<()> {
    require_fact_origin(event, EventType::FactInvalidated)?;
    let EventPayload::Fact(FactPayload::Invalidated(value)) = event.payload() else {
        return Err(fact_error("fact_projection_payload_mismatch"));
    };
    let data = value.invalidation();
    let key = fact_tombstone_key(&data.scope, &data.key, &data.source_snapshot_id)?;
    let payload =
        serde_json::to_vec(data).map_err(|_| fact_error("fact_projection_encode_failed"))?;
    if entry.namespace() != FACT_TOMBSTONE_NAMESPACE
        || entry.entry_key() != key
        || entry.ledger_sequence() != event.sequence()
        || entry.payload() != payload
    {
        return Err(fact_error("fact_tombstone_projection_source_mismatch"));
    }
    Ok(())
}

fn verify_sources(
    data: &FactInvalidationEventData,
    published: &PersistedEvent,
    trigger: &PersistedEvent,
    event: &PersistedEvent,
) -> RuntimeStateResult<()> {
    require_fact_origin(published, EventType::FactPublished)?;
    require_fact_origin(event, EventType::FactInvalidated)?;
    if !matches!(event.payload(), EventPayload::Fact(FactPayload::Invalidated(value)) if value.invalidation() == data)
        || trigger.event_id() != &data.invalidated_by_event_id
        || trigger.event_type() != data.invalidated_by_event_type
        || trigger.timestamp_unix_ms() != data.invalidated_at_unix_ms
        || published.sequence() >= trigger.sequence()
        || trigger.sequence() >= event.sequence()
    {
        return Err(fact_error("fact_invalidation_source_mismatch"));
    }
    let EventPayload::Fact(FactPayload::Published(value)) = published.payload() else {
        return Err(fact_error("fact_projection_payload_mismatch"));
    };
    let input = matches!(
        trigger.event_type(),
        EventType::InputCommitted | EventType::InputFailed
    );
    let scope_matches = match trigger.links().instance_id() {
        Some(instance) => {
            value.scope_instances().contains(instance)
                || (value.scope_instances().is_empty() && !input)
        }
        None => !input,
    };
    if !scope_matches
        || !value.records().any(|record| {
            record.scope == data.scope
                && record.key == data.key
                && record.source_snapshot_id == data.source_snapshot_id
                && record.invalidate_on.contains(&trigger.event_type())
        })
    {
        return Err(fact_error("fact_invalidation_target_mismatch"));
    }
    Ok(())
}

fn require_fact_origin(event: &PersistedEvent, event_type: EventType) -> RuntimeStateResult<()> {
    if event.event_type() != event_type
        || event.origin().source() != EventSource::Runtime
        || event.origin().actor() != EventActor::Runtime
        || event.origin().module() != OriginModule::FactStore
    {
        return Err(fact_error("fact_projection_origin_invalid"));
    }
    Ok(())
}

fn verify_original(
    database: &RuntimeDatabase,
    scope: &RuntimeTransaction<'_, '_>,
    event: &PersistedEvent,
) -> RuntimeStateResult<()> {
    actingcommand_ledger::verify_transaction_event(database, scope, event).map_err(|error| {
        fact_error(error.code()).with_detail(format!("{error}; detail={:?}", error.detail()))
    })
}

fn fact_error(code: &'static str) -> RuntimeStateError {
    fatal(code, "persist_fact_projection")
}

fn fact_sql_error(operation: &'static str, error: &rusqlite::Error) -> RuntimeStateError {
    fatal("fact_projection_transaction_failed", operation).with_detail(error.to_string())
}
