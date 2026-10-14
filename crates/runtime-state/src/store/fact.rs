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
                .map_err(|_| fact_error(StateCode::FactProjectionWorkPoisoned.code()))? =
                Some(event.clone());
            return Ok(());
        }
        if current != self.baseline {
            return Err(fact_error(StateCode::FactTombstoneProjectionChanged.code()));
        }
        *self
            .applied
            .lock()
            .map_err(|_| fact_error(StateCode::FactProjectionWorkPoisoned.code()))? =
            Some(event.clone());
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
            .map_err(|_| fact_error(StateCode::FactProjectionWorkPoisoned.code()))?;
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
            return Err(fact_error(StateCode::StateTransactionOwnerMismatch.code()));
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
                    .validate_projection_row(row, StateLocation::ApplyFactProjection.location())
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
        let payload = serde_json::to_vec(data)
            .map_err(|_| fact_error(StateCode::FactProjectionEncodeFailed.code()))?;
        validate_projection_input(FACT_TOMBSTONE_NAMESPACE, &key, 1, &payload)?;
        let baseline = self.read_projection_entry(FACT_TOMBSTONE_NAMESPACE, &key)?;
        if baseline.is_some() {
            return Err(fact_error(
                StateCode::FactTombstoneProjectionSourceUnproven.code(),
            ));
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
        let mut connection = self.connection(StateLocation::VerifyFactProjection.location())?;
        let transaction = connection.transaction().map_err(|error| {
            fact_sql_error(StateLocation::BeginFactVerification.location(), &error)
        })?;
        let scope = self.database.borrow_transaction(&transaction);
        verify_original(&self.database, &scope, event)?;
        let current =
            query_projection_entry(&transaction, FACT_TOMBSTONE_NAMESPACE, entry.entry_key())?
                .map(|row| {
                    self.validate_projection_row(
                        row,
                        StateLocation::VerifyFactProjection.location(),
                    )
                })
                .transpose()?;
        if current.as_ref() != Some(entry) {
            return Err(fact_error(StateCode::FactTombstoneProjectionChanged.code()));
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
                return Err(fact_error(
                    StateCode::FactProjectionHistoryOrderInvalid.code(),
                ));
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
                        .ok_or_else(|| {
                            fact_error(StateCode::FactInvalidationPublicationMissing.code())
                        })?;
                    let trigger =
                        originals
                            .get(&data.invalidated_by_event_id)
                            .ok_or_else(|| {
                                fact_error(StateCode::FactInvalidationTriggerMissing.code())
                            })?;
                    verify_sources(data, source, trigger, event)?;
                    let key = fact_tombstone_key(&data.scope, &data.key, &data.source_snapshot_id)?;
                    if let Some(prior) = latest.insert(key, event)
                        && !matches!(prior.payload(), EventPayload::Fact(FactPayload::Invalidated(value)) if value.invalidation() == data)
                    {
                        return Err(fact_error(
                            SharedCode::FactInvalidationIdentityConflict.code(),
                        ));
                    }
                    facts.insert(event.sequence(), (event, *source, *trigger));
                }
                _ => {}
            }
            originals.insert(*event.event_id(), event);
        }
        let mut connection = self.connection(StateLocation::RecoverFactProjections.location())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| fact_sql_error(StateLocation::BeginFactRecovery.location(), &error))?;
        let work = (|| {
            let scope = self.database.borrow_transaction(&transaction);
            for (event, published, trigger) in facts.values() {
                for original in [event, published, trigger] {
                    verify_original(&self.database, &scope, original)?;
                }
            }
            let keys = {
                let mut statement = transaction.prepare("SELECT entry_key FROM projection_entries WHERE namespace=?1 ORDER BY entry_key")
                    .map_err(|error| fact_sql_error(StateLocation::ReadFactProjectionKeys.location(), &error))?;
                statement
                    .query_map([FACT_TOMBSTONE_NAMESPACE], |row| row.get::<_, String>(0))
                    .map_err(|error| {
                        fact_sql_error(StateLocation::ReadFactProjectionKeys.location(), &error)
                    })?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| {
                        fact_sql_error(StateLocation::ReadFactProjectionKeys.location(), &error)
                    })?
            };
            for key in keys {
                let row = query_projection_entry(&transaction, FACT_TOMBSTONE_NAMESPACE, &key)?
                    .ok_or_else(|| fact_error(SharedCode::FactTombstoneProjectionMissing.code()))?;
                let entry = self.validate_projection_row(
                    row,
                    StateLocation::RecoverFactProjections.location(),
                )?;
                let (event, _, _) = facts.get(&entry.ledger_sequence()).ok_or_else(|| {
                    fact_error(StateCode::FactTombstoneProjectionSourceUnproven.code())
                })?;
                require_entry(&entry, event)?;
            }
            for (key, event) in latest {
                let EventPayload::Fact(FactPayload::Invalidated(value)) = event.payload() else {
                    return Err(fact_error(StateCode::FactProjectionPayloadMismatch.code()));
                };
                let payload = serde_json::to_vec(value.invalidation())
                    .map_err(|_| fact_error(StateCode::FactProjectionEncodeFailed.code()))?;
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
                Err(rollback) => Err(fact_error(StateCode::FactProjectionRollbackFailed.code())
                    .with_detail(format!("primary={original}; rollback={rollback}"))),
            };
        }
        transaction.commit().map_err(|error| {
            fact_sql_error(StateLocation::CommitFactRecovery.location(), &error)
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
        .map_err(|_| fact_error(StateCode::FactTombstoneIdentityEncodeFailed.code()))?;
    Ok(format!("{:x}", Sha256::digest(identity)))
}

fn require_entry(entry: &ProjectionEntry, event: &PersistedEvent) -> RuntimeStateResult<()> {
    require_fact_origin(event, EventType::FactInvalidated)?;
    let EventPayload::Fact(FactPayload::Invalidated(value)) = event.payload() else {
        return Err(fact_error(StateCode::FactProjectionPayloadMismatch.code()));
    };
    let data = value.invalidation();
    let key = fact_tombstone_key(&data.scope, &data.key, &data.source_snapshot_id)?;
    let payload = serde_json::to_vec(data)
        .map_err(|_| fact_error(StateCode::FactProjectionEncodeFailed.code()))?;
    if entry.namespace() != FACT_TOMBSTONE_NAMESPACE
        || entry.entry_key() != key
        || entry.ledger_sequence() != event.sequence()
        || entry.payload() != payload
    {
        return Err(fact_error(
            SharedCode::FactTombstoneProjectionSourceMismatch.code(),
        ));
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
        return Err(fact_error(
            SharedCode::FactInvalidationSourceMismatch.code(),
        ));
    }
    let EventPayload::Fact(FactPayload::Published(value)) = published.payload() else {
        return Err(fact_error(StateCode::FactProjectionPayloadMismatch.code()));
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
    // Workflow #384 D2-S3: an adjustment's invalidation names its operation's step as the
    // trigger, which the record does not list.
    let adjustment = actingcommand_contract::is_adjustment_invalidation(data);
    if !scope_matches
        || !value.records().any(|record| {
            record.scope == data.scope
                && record.key == data.key
                && record.source_snapshot_id == data.source_snapshot_id
                && (adjustment || record.invalidate_on.contains(&trigger.event_type()))
        })
    {
        return Err(fact_error(
            SharedCode::FactInvalidationTargetMismatch.code(),
        ));
    }
    Ok(())
}

fn require_fact_origin(event: &PersistedEvent, event_type: EventType) -> RuntimeStateResult<()> {
    if event.event_type() != event_type
        || event.origin().source() != EventSource::Runtime
        || event.origin().actor() != EventActor::Runtime
        || event.origin().module() != OriginModule::FactStore
    {
        return Err(fact_error(StateCode::FactProjectionOriginInvalid.code()));
    }
    Ok(())
}

fn verify_original(
    database: &RuntimeDatabase,
    scope: &RuntimeTransaction<'_, '_>,
    event: &PersistedEvent,
) -> RuntimeStateResult<()> {
    actingcommand_ledger::verify_transaction_event(database, scope, event).map_err(|error| {
        RuntimeStateError::relayed_fatal(
            error.code(),
            StateLocation::PersistFactProjection.as_str(),
        )
        .with_detail(format!("{error}; detail={:?}", error.detail()))
    })
}

fn fact_error(code: Code) -> RuntimeStateError {
    fatal(code, StateLocation::PersistFactProjection.location())
}

fn fact_sql_error(operation: Location, error: &rusqlite::Error) -> RuntimeStateError {
    fatal(StateCode::FactProjectionTransactionFailed.code(), operation)
        .with_detail(error.to_string())
}
