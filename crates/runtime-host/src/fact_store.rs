// SPDX-License-Identifier: AGPL-3.0-only

//! Ledger-rebuilt fact projection shared by policy and execution boundaries.

use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::{
    EventId, EventPayload, EventQuery, EventType, FactContent, FactInvalidationEventData,
    FactPayload, FactRecord, FactScalar as ContractFactScalar, FactScope,
    FactValue as ContractFactValue, InstanceFactContext, InstanceFactSnapshot, RuntimeErrorCode,
};
use actingcommand_ledger::{GlobalLedger, PersistedEvent};
use actingcommand_policy::{
    EvaluationFacts, FactScalar as PolicyFactScalar, FactValue as PolicyFactValue,
    InstanceSnapshot, ObservedFact, ScopeSelector,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

type FactIdentity = (FactScope, String);
type InvalidationIdentity = (FactScope, String, String, EventId);
const MAX_ACTIVE_FACTS: usize = 256;
const MAX_FACT_TOMBSTONES: usize = 65_536;

#[derive(Clone)]
struct StoredFact {
    record: FactRecord,
    sequence: u64,
    event_id: EventId,
}

#[derive(Clone)]
struct InvalidationTombstone {
    data: FactInvalidationEventData,
}

#[derive(Clone)]
pub(crate) struct InstanceFactStore {
    active: BTreeMap<FactIdentity, StoredFact>,
    invalidated: BTreeMap<(FactIdentity, String), InvalidationTombstone>,
    pending: BTreeMap<InvalidationIdentity, FactInvalidationEventData>,
    last_sequence: u64,
}

impl InstanceFactStore {
    pub(crate) fn recover(ledger: &GlobalLedger) -> RuntimeHostResult<Self> {
        let mut store = Self {
            active: BTreeMap::new(),
            invalidated: BTreeMap::new(),
            pending: BTreeMap::new(),
            last_sequence: 0,
        };
        let events = ledger
            .query(Default::default())
            .map_err(|_| fact_fatal("fact_store_recovery_failed", "recover_fact_store"))?;
        for event in events {
            store.replay_event(&event)?;
        }
        Ok(store)
    }

    pub(crate) fn synchronize(&mut self, ledger: &GlobalLedger) -> RuntimeHostResult<()> {
        let from_sequence = self
            .last_sequence
            .checked_add(1)
            .ok_or_else(|| fact_fatal("fact_sequence_overflow", "synchronize_fact_store"))?;
        let events = ledger
            .query(EventQuery {
                from_sequence: Some(from_sequence),
                ..EventQuery::default()
            })
            .map_err(|_| fact_fatal("fact_store_sync_failed", "synchronize_fact_store"))?;
        for event in events {
            self.replay_event(&event)?;
        }
        Ok(())
    }

    pub(crate) fn pending_invalidations(&self) -> Vec<FactInvalidationEventData> {
        self.pending.values().cloned().collect()
    }

    fn replay_event(&mut self, event: &PersistedEvent) -> RuntimeHostResult<()> {
        match event.payload() {
            EventPayload::Fact(FactPayload::Published(payload)) => self.commit_publish(
                payload.record().clone(),
                event.sequence(),
                *event.event_id(),
            ),
            EventPayload::Fact(FactPayload::Invalidated(payload)) => {
                self.commit_invalidation(payload.invalidation().clone(), event.sequence())
            }
            _ => {
                for invalidation in self.plan_invalidations(event) {
                    self.derive_invalidation(invalidation, event.sequence())?;
                }
                self.advance(event.sequence(), "replay_fact_event")
            }
        }
    }

    pub(crate) fn preview_publish(
        &self,
        record: &FactRecord,
    ) -> RuntimeHostResult<Option<EventId>> {
        record
            .validate()
            .map_err(|_| fact_request("fact_record_invalid", "publish_fact"))?;
        let identity = (record.scope.clone(), record.key.clone());
        if self
            .invalidated
            .contains_key(&(identity.clone(), record.source_snapshot_id.clone()))
        {
            return Err(fact_request(
                "fact_source_snapshot_invalidated",
                "publish_fact",
            ));
        }
        let Some(existing) = self.active.get(&identity) else {
            if self.active.len() >= MAX_ACTIVE_FACTS {
                return Err(fact_request("fact_store_capacity_exceeded", "publish_fact"));
            }
            return Ok(None);
        };
        if existing.record.source_snapshot_id != record.source_snapshot_id {
            return Ok(None);
        }
        if existing.record == *record {
            return Ok(Some(existing.event_id));
        }
        Err(fact_fatal(
            "fact_source_snapshot_identity_conflict",
            "publish_fact",
        ))
    }

    pub(crate) fn commit_publish(
        &mut self,
        record: FactRecord,
        sequence: u64,
        event_id: EventId,
    ) -> RuntimeHostResult<()> {
        record
            .validate()
            .map_err(|_| fact_fatal("fact_record_invalid", "commit_fact"))?;
        if sequence == 0 {
            return Err(fact_fatal("fact_sequence_invalid", "commit_fact"));
        }
        let identity = (record.scope.clone(), record.key.clone());
        if self
            .invalidated
            .contains_key(&(identity.clone(), record.source_snapshot_id.clone()))
        {
            return Err(fact_fatal(
                "fact_source_snapshot_republished",
                "commit_fact",
            ));
        }
        if !self.active.contains_key(&identity) && self.active.len() >= MAX_ACTIVE_FACTS {
            return Err(fact_fatal("fact_store_capacity_exceeded", "commit_fact"));
        }
        if let Some(existing) = self.active.get(&identity)
            && existing.record.source_snapshot_id == record.source_snapshot_id
        {
            if existing.record != record || existing.event_id != event_id {
                return Err(fact_fatal(
                    "fact_source_snapshot_identity_conflict",
                    "commit_fact",
                ));
            }
            return self.advance(sequence, "commit_fact");
        }
        self.active.insert(
            identity,
            StoredFact {
                record,
                sequence,
                event_id,
            },
        );
        self.advance(sequence, "commit_fact")
    }

    pub(crate) fn plan_invalidations(
        &self,
        event: &PersistedEvent,
    ) -> Vec<FactInvalidationEventData> {
        if matches!(
            event.event_type(),
            EventType::FactPublished | EventType::FactInvalidated
        ) {
            return Vec::new();
        }
        self.active
            .values()
            .filter(|stored| {
                stored.sequence <= event.sequence()
                    && stored.record.invalidate_on.contains(&event.event_type())
            })
            .map(|stored| FactInvalidationEventData {
                scope: stored.record.scope.clone(),
                key: stored.record.key.clone(),
                source_snapshot_id: stored.record.source_snapshot_id.clone(),
                invalidated_at_unix_ms: event.timestamp_unix_ms(),
                invalidated_by_event_id: *event.event_id(),
                invalidated_by_event_type: event.event_type(),
            })
            .collect()
    }

    pub(crate) fn commit_invalidation(
        &mut self,
        data: FactInvalidationEventData,
        sequence: u64,
    ) -> RuntimeHostResult<()> {
        self.apply_invalidation(data, sequence, true)
    }

    pub(crate) fn acknowledge_generated_invalidation(
        &mut self,
        data: &FactInvalidationEventData,
    ) -> RuntimeHostResult<()> {
        let tombstone_key = (
            (data.scope.clone(), data.key.clone()),
            data.source_snapshot_id.clone(),
        );
        let existing = self.invalidated.get(&tombstone_key).ok_or_else(|| {
            fact_fatal(
                "fact_generated_invalidation_missing",
                "acknowledge_fact_invalidation",
            )
        })?;
        let identity = invalidation_identity(data);
        if existing.data != *data || self.pending.remove(&identity).is_none() {
            return Err(fact_fatal(
                "fact_generated_invalidation_mismatch",
                "acknowledge_fact_invalidation",
            ));
        }
        Ok(())
    }

    fn derive_invalidation(
        &mut self,
        data: FactInvalidationEventData,
        sequence: u64,
    ) -> RuntimeHostResult<()> {
        self.apply_invalidation(data, sequence, false)
    }

    fn apply_invalidation(
        &mut self,
        data: FactInvalidationEventData,
        sequence: u64,
        persisted: bool,
    ) -> RuntimeHostResult<()> {
        let identity = (data.scope.clone(), data.key.clone());
        let tombstone_key = (identity.clone(), data.source_snapshot_id.clone());
        if let Some(existing) = self.invalidated.get(&tombstone_key) {
            if existing.data != data {
                return Err(fact_fatal(
                    "fact_invalidation_identity_conflict",
                    "commit_fact_invalidation",
                ));
            }
            if persisted {
                self.pending.remove(&invalidation_identity(&data));
            }
            return self.advance(sequence, "commit_fact_invalidation");
        }
        let active = self.active.get(&identity).ok_or_else(|| {
            fact_fatal(
                "fact_invalidation_target_missing",
                "commit_fact_invalidation",
            )
        })?;
        if active.record.source_snapshot_id != data.source_snapshot_id
            || !active
                .record
                .invalidate_on
                .contains(&data.invalidated_by_event_type)
        {
            return Err(fact_fatal(
                "fact_invalidation_target_mismatch",
                "commit_fact_invalidation",
            ));
        }
        if self.invalidated.len() >= MAX_FACT_TOMBSTONES {
            return Err(fact_fatal(
                "fact_tombstone_capacity_exceeded",
                "commit_fact_invalidation",
            ));
        }
        self.active.remove(&identity);
        self.invalidated
            .insert(tombstone_key, InvalidationTombstone { data: data.clone() });
        if !persisted {
            self.pending.insert(invalidation_identity(&data), data);
        }
        self.advance(sequence, "commit_fact_invalidation")
    }

    #[cfg(test)]
    fn apply_trigger(&mut self, event_type: EventType, event_id: EventId, at: u64) {
        let invalidations = self
            .active
            .values()
            .filter(|stored| stored.record.invalidate_on.contains(&event_type))
            .map(|stored| FactInvalidationEventData {
                scope: stored.record.scope.clone(),
                key: stored.record.key.clone(),
                source_snapshot_id: stored.record.source_snapshot_id.clone(),
                invalidated_at_unix_ms: at,
                invalidated_by_event_id: event_id,
                invalidated_by_event_type: event_type,
            })
            .collect::<Vec<_>>();
        for data in invalidations {
            self.derive_invalidation(data, self.last_sequence + 1)
                .expect("test trigger invalidation");
        }
    }

    fn advance(&mut self, sequence: u64, operation: &'static str) -> RuntimeHostResult<()> {
        if sequence == 0 || sequence < self.last_sequence {
            return Err(fact_fatal("fact_sequence_invalid", operation));
        }
        self.last_sequence = sequence;
        Ok(())
    }

    pub(crate) fn snapshot(
        &self,
        context: InstanceFactContext,
        ledger_position: u64,
    ) -> RuntimeHostResult<InstanceFactSnapshot> {
        context
            .validate()
            .map_err(|_| fact_request("fact_context_invalid", "read_fact_snapshot"))?;
        if ledger_position == 0 {
            return Err(fact_fatal(
                "fact_ledger_position_invalid",
                "read_fact_snapshot",
            ));
        }
        let mut records = self
            .active
            .values()
            .filter(|stored| stored.record.scope.matches(&context))
            .map(|stored| stored.record.clone())
            .collect::<Vec<_>>();
        records.sort_by(|left, right| {
            left.key
                .cmp(&right.key)
                .then_with(|| left.scope.cmp(&right.scope))
        });
        let snapshot_id = snapshot_id(&context, ledger_position, &records)?;
        let snapshot = InstanceFactSnapshot {
            snapshot_id,
            ledger_position,
            context,
            records,
        };
        snapshot
            .validate()
            .map_err(|_| fact_fatal("fact_snapshot_invalid", "read_fact_snapshot"))?;
        Ok(snapshot)
    }

    pub(crate) fn active_records(&self) -> Vec<FactRecord> {
        let mut records = self
            .active
            .values()
            .map(|stored| stored.record.clone())
            .collect::<Vec<_>>();
        records.sort_by(|left, right| {
            left.scope
                .cmp(&right.scope)
                .then_with(|| left.key.cmp(&right.key))
        });
        records
    }

    pub(crate) fn overlay_policy_facts(
        &self,
        facts: &EvaluationFacts,
        ledger_position: u64,
    ) -> RuntimeHostResult<EvaluationFacts> {
        if self.active.is_empty() {
            return Ok(facts.clone());
        }
        let mut projected = facts.clone();
        let contexts = projected
            .instances
            .iter()
            .map(instance_context)
            .collect::<Vec<_>>();
        let mut seen = projected
            .facts
            .iter()
            .map(|fact| policy_fact_identity(&fact.scope, &fact.fact_key))
            .collect::<BTreeSet<_>>();
        let mut selected = BTreeMap::<FactIdentity, &StoredFact>::new();
        for stored in self.active.values() {
            if contexts
                .iter()
                .any(|context| stored.record.scope.matches(context))
            {
                selected.insert(
                    (stored.record.scope.clone(), stored.record.key.clone()),
                    stored,
                );
            }
        }
        if selected.is_empty() {
            return Ok(facts.clone());
        }
        for (identity, stored) in &selected {
            let scope = policy_scope(&identity.0);
            let fact_key = identity.1.clone();
            if !seen.insert(policy_fact_identity(&scope, &fact_key)) {
                return Err(fact_fatal(
                    "policy_fact_authority_conflict",
                    "project_policy_facts",
                ));
            }
            let FactContent::Inline { value } = &stored.record.content else {
                continue;
            };
            projected.facts.push(ObservedFact {
                scope,
                fact_key,
                value: policy_value(value),
                observed_at_unix_ms: stored.record.observed_at_unix_ms,
                expires_at_unix_ms: stored.record.expires_at_unix_ms,
                confidence_milli: stored.record.confidence_milli,
            });
        }
        projected.facts.sort_by(|left, right| {
            policy_scope_key(&left.scope)
                .cmp(&policy_scope_key(&right.scope))
                .then_with(|| left.fact_key.cmp(&right.fact_key))
        });
        projected.ledger_position = ledger_position;
        projected.fact_snapshot_id = combined_policy_snapshot_id(ledger_position, &selected)?;
        Ok(projected)
    }

    #[cfg(test)]
    fn active_count(&self) -> usize {
        self.active.len()
    }
}

fn invalidation_identity(data: &FactInvalidationEventData) -> InvalidationIdentity {
    (
        data.scope.clone(),
        data.key.clone(),
        data.source_snapshot_id.clone(),
        data.invalidated_by_event_id,
    )
}

fn instance_context(instance: &InstanceSnapshot) -> InstanceFactContext {
    InstanceFactContext {
        instance_id: instance.instance_id.clone(),
        server_id: instance.server_id.clone(),
        game_id: instance.game_id.clone(),
    }
}

fn policy_scope(scope: &FactScope) -> ScopeSelector {
    match scope {
        FactScope::Instance { instance_id } => ScopeSelector::Instance {
            instance_id: instance_id.clone(),
        },
        FactScope::Server { server_id } => ScopeSelector::Server {
            server_id: server_id.clone(),
        },
        FactScope::Game { game_id } => ScopeSelector::Game {
            game_id: game_id.clone(),
        },
    }
}

fn policy_fact_identity(scope: &ScopeSelector, fact_key: &str) -> String {
    format!("{}\u{1f}{fact_key}", policy_scope_key(scope))
}

fn policy_value(value: &ContractFactValue) -> PolicyFactValue {
    match value {
        ContractFactValue::Boolean(value) => PolicyFactValue::Boolean(*value),
        ContractFactValue::Integer(value) => PolicyFactValue::Integer(*value),
        ContractFactValue::String(value) => PolicyFactValue::String(value.clone()),
        ContractFactValue::TimestampMs(value) => PolicyFactValue::TimestampMs(*value),
        ContractFactValue::DurationMs(value) => PolicyFactValue::DurationMs(*value),
        ContractFactValue::RecordList(records) => PolicyFactValue::RecordList(
            records
                .iter()
                .map(|record| {
                    record
                        .iter()
                        .map(|(key, value)| (key.clone(), policy_scalar(value)))
                        .collect()
                })
                .collect(),
        ),
    }
}

fn policy_scalar(value: &ContractFactScalar) -> PolicyFactScalar {
    match value {
        ContractFactScalar::Boolean(value) => PolicyFactScalar::Boolean(*value),
        ContractFactScalar::Integer(value) => PolicyFactScalar::Integer(*value),
        ContractFactScalar::String(value) => PolicyFactScalar::String(value.clone()),
        ContractFactScalar::TimestampMs(value) => PolicyFactScalar::TimestampMs(*value),
        ContractFactScalar::DurationMs(value) => PolicyFactScalar::DurationMs(*value),
    }
}

fn policy_scope_key(scope: &ScopeSelector) -> String {
    match scope {
        ScopeSelector::Instance { instance_id } => format!("instance:{instance_id}"),
        ScopeSelector::Server { server_id } => format!("server:{server_id}"),
        ScopeSelector::Game { game_id } => format!("game:{game_id}"),
    }
}

fn snapshot_id(
    context: &InstanceFactContext,
    ledger_position: u64,
    records: &[FactRecord],
) -> RuntimeHostResult<String> {
    let bytes = serde_json::to_vec(&(context, ledger_position, records))
        .map_err(|_| fact_fatal("fact_snapshot_encode_failed", "hash_fact_snapshot"))?;
    Ok(format!("snapshot:fact:{:x}", Sha256::digest(bytes)))
}

fn combined_policy_snapshot_id(
    ledger_position: u64,
    facts: &BTreeMap<FactIdentity, &StoredFact>,
) -> RuntimeHostResult<String> {
    let records = facts
        .values()
        .map(|stored| (&stored.record, stored.sequence))
        .collect::<Vec<_>>();
    let bytes = serde_json::to_vec(&(ledger_position, records))
        .map_err(|_| fact_fatal("fact_snapshot_encode_failed", "hash_policy_fact_snapshot"))?;
    Ok(format!("snapshot:policy-fact:{:x}", Sha256::digest(bytes)))
}

fn fact_request(code: &'static str, operation: &'static str) -> RuntimeHostError {
    RuntimeHostError::request(code, operation, RuntimeErrorCode::InvalidRequest)
}

fn fact_fatal(code: &'static str, operation: &'static str) -> RuntimeHostError {
    RuntimeHostError::fatal(code, operation, RuntimeErrorCode::RuntimeFatal)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(scope: FactScope, snapshot: &str, invalidate_on: Vec<EventType>) -> FactRecord {
        FactRecord {
            scope,
            key: "env.theme".to_owned(),
            content: FactContent::Inline {
                value: ContractFactValue::String("Neutral".to_owned()),
            },
            observed_at_unix_ms: 1_000,
            expires_at_unix_ms: Some(5_000),
            confidence_milli: 900,
            source_detector: "detector.theme".to_owned(),
            source_snapshot_id: snapshot.to_owned(),
            schema_version: "fact.v1".to_owned(),
            resource_bundle_hash: "a".repeat(64),
            invalidate_on,
        }
    }

    #[test]
    fn server_fact_is_shared_and_instance_fact_remains_isolated() {
        let mut store = InstanceFactStore {
            active: BTreeMap::new(),
            invalidated: BTreeMap::new(),
            pending: BTreeMap::new(),
            last_sequence: 0,
        };
        let issuer = actingcommand_contract::IdentifierIssuer::new().expect("issuer");
        store
            .commit_publish(
                record(
                    FactScope::Server {
                        server_id: "server-a".to_owned(),
                    },
                    "snapshot:server",
                    Vec::new(),
                ),
                1,
                *issuer.mint_event_id().expect("event").transport(),
            )
            .expect("server fact");
        store
            .commit_publish(
                record(
                    FactScope::Instance {
                        instance_id: "instance-a".to_owned(),
                    },
                    "snapshot:instance",
                    Vec::new(),
                ),
                2,
                *issuer.mint_event_id().expect("event").transport(),
            )
            .expect("instance fact");
        let a = store
            .snapshot(
                InstanceFactContext {
                    instance_id: "instance-a".to_owned(),
                    server_id: "server-a".to_owned(),
                    game_id: "game-a".to_owned(),
                },
                2,
            )
            .expect("snapshot a");
        let b = store
            .snapshot(
                InstanceFactContext {
                    instance_id: "instance-b".to_owned(),
                    server_id: "server-a".to_owned(),
                    game_id: "game-a".to_owned(),
                },
                2,
            )
            .expect("snapshot b");
        assert_eq!(a.records.len(), 2);
        assert_eq!(b.records.len(), 1);
    }

    #[test]
    fn event_invalidation_removes_only_the_matching_snapshot() {
        let mut store = InstanceFactStore {
            active: BTreeMap::new(),
            invalidated: BTreeMap::new(),
            pending: BTreeMap::new(),
            last_sequence: 0,
        };
        let issuer = actingcommand_contract::IdentifierIssuer::new().expect("issuer");
        let published = *issuer.mint_event_id().expect("event").transport();
        let trigger = *issuer.mint_event_id().expect("event").transport();
        store
            .commit_publish(
                record(
                    FactScope::Instance {
                        instance_id: "instance-a".to_owned(),
                    },
                    "snapshot:instance",
                    vec![EventType::RuntimeTakeover],
                ),
                1,
                published,
            )
            .expect("publish");
        store.apply_trigger(EventType::RuntimeTakeover, trigger, 2_000);
        assert_eq!(store.active_count(), 0);
    }
}
