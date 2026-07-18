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
    EvaluationFacts, EvaluationResources, FactScalar as PolicyFactScalar,
    FactValue as PolicyFactValue, InstanceSnapshot, ObservedFact, ScopeSelector,
};
use actingcommand_runtime_state::RuntimeStateStore;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

type FactIdentity = (FactScope, String);
type InvalidationIdentity = (FactScope, String, String, EventId);
type HistoricalInvalidationIdentity = (FactIdentity, String);
const MAX_ACTIVE_FACTS: usize = 256;
const FACT_TOMBSTONE_NAMESPACE: &str = "fact.tombstone.v1";
const MAX_RECENT_FACT_TOMBSTONES: usize = 256;

#[derive(Clone, Copy)]
enum PolicyFactConflictDisposition {
    FatalInvariant,
    RejectCaller,
}

#[derive(Clone)]
struct StoredFact {
    record: FactRecord,
    sequence: u64,
    event_id: EventId,
}

#[derive(Clone)]
struct InvalidationTombstone {
    data: FactInvalidationEventData,
    sequence: u64,
}

#[derive(Default)]
struct HistoricalFactProjection {
    active: BTreeMap<FactIdentity, (FactRecord, EventId)>,
    invalidated: BTreeMap<HistoricalInvalidationIdentity, FactInvalidationEventData>,
}

impl HistoricalFactProjection {
    fn replay(&mut self, event: &PersistedEvent) -> RuntimeHostResult<()> {
        match event.payload() {
            EventPayload::Fact(FactPayload::Published(payload)) => {
                self.publish(payload.record().clone(), *event.event_id())
            }
            EventPayload::Fact(FactPayload::Invalidated(payload)) => {
                self.invalidate(payload.invalidation().clone())
            }
            _ => {
                let invalidations = self
                    .active
                    .values()
                    .filter(|(record, _)| record.invalidate_on.contains(&event.event_type()))
                    .map(|(record, _)| FactInvalidationEventData {
                        scope: record.scope.clone(),
                        key: record.key.clone(),
                        source_snapshot_id: record.source_snapshot_id.clone(),
                        invalidated_at_unix_ms: event.timestamp_unix_ms(),
                        invalidated_by_event_id: *event.event_id(),
                        invalidated_by_event_type: event.event_type(),
                    })
                    .collect::<Vec<_>>();
                for invalidation in invalidations {
                    self.invalidate(invalidation)?;
                }
                Ok(())
            }
        }
    }

    fn publish(&mut self, record: FactRecord, event_id: EventId) -> RuntimeHostResult<()> {
        record
            .validate()
            .map_err(|_| fact_fatal("fact_record_invalid", "project_fact_history"))?;
        let identity = (record.scope.clone(), record.key.clone());
        if self
            .invalidated
            .contains_key(&(identity.clone(), record.source_snapshot_id.clone()))
        {
            return Err(fact_fatal(
                "fact_source_snapshot_republished",
                "project_fact_history",
            ));
        }
        if let Some((existing, existing_event_id)) = self.active.get(&identity)
            && existing.source_snapshot_id == record.source_snapshot_id
        {
            if existing != &record || existing_event_id != &event_id {
                return Err(fact_fatal(
                    "fact_source_snapshot_identity_conflict",
                    "project_fact_history",
                ));
            }
            return Ok(());
        }
        if !self.active.contains_key(&identity) && self.active.len() >= MAX_ACTIVE_FACTS {
            return Err(fact_fatal(
                "fact_store_capacity_exceeded",
                "project_fact_history",
            ));
        }
        self.active.insert(identity, (record, event_id));
        Ok(())
    }

    fn invalidate(&mut self, data: FactInvalidationEventData) -> RuntimeHostResult<()> {
        let identity = (data.scope.clone(), data.key.clone());
        let invalidation_identity = (identity.clone(), data.source_snapshot_id.clone());
        if let Some(existing) = self.invalidated.get(&invalidation_identity) {
            if existing != &data {
                return Err(fact_fatal(
                    "fact_invalidation_identity_conflict",
                    "project_fact_history",
                ));
            }
            return Ok(());
        }
        let (active, _) = self.active.get(&identity).ok_or_else(|| {
            fact_fatal("fact_invalidation_target_missing", "project_fact_history")
        })?;
        if active.source_snapshot_id != data.source_snapshot_id
            || !active
                .invalidate_on
                .contains(&data.invalidated_by_event_type)
        {
            return Err(fact_fatal(
                "fact_invalidation_target_mismatch",
                "project_fact_history",
            ));
        }
        self.active.remove(&identity);
        self.invalidated.insert(invalidation_identity, data);
        Ok(())
    }

    fn records(self) -> Vec<FactRecord> {
        self.active
            .into_values()
            .map(|(record, _)| record)
            .collect()
    }
}

#[derive(Clone)]
pub(crate) struct InstanceFactStore {
    active: BTreeMap<FactIdentity, StoredFact>,
    invalidated: BTreeMap<(FactIdentity, String), InvalidationTombstone>,
    pending: BTreeMap<InvalidationIdentity, FactInvalidationEventData>,
    last_sequence: u64,
    state: Arc<RuntimeStateStore>,
}

impl InstanceFactStore {
    pub(crate) fn active_records_at(
        ledger: &GlobalLedger,
        ledger_position: u64,
    ) -> RuntimeHostResult<Vec<FactRecord>> {
        let latest = ledger
            .latest_sequence()
            .map_err(|_| fact_fatal("fact_store_query_failed", "project_fact_history"))?;
        if ledger_position == 0 || ledger_position > latest {
            return Err(fact_fatal(
                "fact_ledger_position_invalid",
                "project_fact_history",
            ));
        }
        let events = ledger
            .query(EventQuery {
                to_sequence: Some(ledger_position),
                ..EventQuery::default()
            })
            .map_err(|_| fact_fatal("fact_store_query_failed", "project_fact_history"))?;
        let mut projection = HistoricalFactProjection::default();
        for event in events {
            projection.replay(&event)?;
        }
        Ok(projection.records())
    }

    pub(crate) fn recover(
        ledger: &GlobalLedger,
        state: Arc<RuntimeStateStore>,
    ) -> RuntimeHostResult<Self> {
        let mut store = Self {
            active: BTreeMap::new(),
            invalidated: BTreeMap::new(),
            pending: BTreeMap::new(),
            last_sequence: 0,
            state,
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
            || self
                .persisted_tombstone(&record.scope, &record.key, &record.source_snapshot_id)?
                .is_some()
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
        if self
            .persisted_tombstone(&record.scope, &record.key, &record.source_snapshot_id)?
            .is_some_and(|tombstone| tombstone.sequence <= sequence)
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
        sequence: u64,
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
        self.persist_tombstone(data, sequence)?;
        let tombstone = self.invalidated.get_mut(&tombstone_key).ok_or_else(|| {
            fact_fatal(
                "fact_generated_invalidation_missing",
                "acknowledge_fact_invalidation",
            )
        })?;
        tombstone.sequence = sequence;
        self.trim_recent_tombstones()?;
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
                self.persist_tombstone(&data, sequence)?;
                let tombstone = self.invalidated.get_mut(&tombstone_key).ok_or_else(|| {
                    fact_fatal(
                        "fact_invalidation_target_missing",
                        "commit_fact_invalidation",
                    )
                })?;
                tombstone.sequence = sequence;
                self.trim_recent_tombstones()?;
            }
            return self.advance(sequence, "commit_fact_invalidation");
        }
        if let Some(existing) =
            self.persisted_tombstone(&data.scope, &data.key, &data.source_snapshot_id)?
            && existing.data != data
        {
            return Err(fact_fatal(
                "fact_invalidation_identity_conflict",
                "commit_fact_invalidation",
            ));
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
        self.active.remove(&identity);
        self.invalidated.insert(
            tombstone_key,
            InvalidationTombstone {
                data: data.clone(),
                sequence,
            },
        );
        if persisted {
            self.persist_tombstone(&data, sequence)?;
            self.trim_recent_tombstones()?;
        } else {
            self.pending.insert(invalidation_identity(&data), data);
        }
        self.advance(sequence, "commit_fact_invalidation")
    }

    fn persisted_tombstone(
        &self,
        scope: &FactScope,
        key: &str,
        source_snapshot_id: &str,
    ) -> RuntimeHostResult<Option<InvalidationTombstone>> {
        let entry_key = fact_tombstone_key(scope, key, source_snapshot_id)?;
        let Some(entry) = self
            .state
            .read_projection_entry(FACT_TOMBSTONE_NAMESPACE, &entry_key)
            .map_err(|error| RuntimeHostError::state(&error))?
        else {
            return Ok(None);
        };
        let data = serde_json::from_slice::<FactInvalidationEventData>(entry.payload())
            .map_err(|_| fact_fatal("fact_tombstone_projection_invalid", "read_fact_tombstone"))?;
        if &data.scope != scope || data.key != key || data.source_snapshot_id != source_snapshot_id
        {
            return Err(fact_fatal(
                "fact_tombstone_projection_identity_mismatch",
                "read_fact_tombstone",
            ));
        }
        Ok(Some(InvalidationTombstone {
            data,
            sequence: entry.ledger_sequence(),
        }))
    }

    fn persist_tombstone(
        &self,
        data: &FactInvalidationEventData,
        sequence: u64,
    ) -> RuntimeHostResult<()> {
        let payload = serde_json::to_vec(data).map_err(|_| {
            fact_fatal(
                "fact_tombstone_projection_encode_failed",
                "persist_fact_tombstone",
            )
        })?;
        self.state
            .write_projection_entry(
                FACT_TOMBSTONE_NAMESPACE,
                &fact_tombstone_key(&data.scope, &data.key, &data.source_snapshot_id)?,
                sequence,
                &payload,
            )
            .map_err(|error| RuntimeHostError::state(&error))?;
        Ok(())
    }

    fn trim_recent_tombstones(&mut self) -> RuntimeHostResult<()> {
        while self.invalidated.len() > MAX_RECENT_FACT_TOMBSTONES {
            let candidate = self
                .invalidated
                .iter()
                .filter(|(_, tombstone)| {
                    !self
                        .pending
                        .contains_key(&invalidation_identity(&tombstone.data))
                })
                .min_by_key(|(identity, tombstone)| (tombstone.sequence, *identity))
                .map(|(identity, _)| identity.clone())
                .ok_or_else(|| {
                    fact_fatal(
                        "fact_tombstone_compaction_blocked",
                        "compact_fact_tombstones",
                    )
                })?;
            self.invalidated.remove(&candidate);
        }
        Ok(())
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

    pub(crate) fn overlay_policy_facts(
        &self,
        facts: &EvaluationFacts,
        resources: &EvaluationResources,
        ledger_position: u64,
    ) -> RuntimeHostResult<EvaluationFacts> {
        self.overlay_policy_facts_with_disposition(
            facts,
            resources,
            ledger_position,
            PolicyFactConflictDisposition::FatalInvariant,
            "project_policy_facts",
        )
    }

    pub(crate) fn overlay_external_policy_facts(
        &self,
        facts: &EvaluationFacts,
        resources: &EvaluationResources,
        ledger_position: u64,
    ) -> RuntimeHostResult<EvaluationFacts> {
        self.overlay_policy_facts_with_disposition(
            facts,
            resources,
            ledger_position,
            PolicyFactConflictDisposition::RejectCaller,
            "project_policy_forward",
        )
    }

    fn overlay_policy_facts_with_disposition(
        &self,
        facts: &EvaluationFacts,
        resources: &EvaluationResources,
        ledger_position: u64,
        conflict_disposition: PolicyFactConflictDisposition,
        operation: &'static str,
    ) -> RuntimeHostResult<EvaluationFacts> {
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
        for (identity, stored) in &selected {
            let scope = policy_scope(&identity.0);
            let fact_key = identity.1.clone();
            if !seen.insert(policy_fact_identity(&scope, &fact_key)) {
                return Err(match conflict_disposition {
                    PolicyFactConflictDisposition::FatalInvariant => {
                        fact_fatal("policy_fact_authority_conflict", operation)
                    }
                    PolicyFactConflictDisposition::RejectCaller => RuntimeHostError::request(
                        "policy_fact_authority_conflict",
                        operation,
                        RuntimeErrorCode::InvalidRequest,
                    ),
                });
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
        projected.outcomes.sort_by(|left, right| {
            (&left.instance_id, &left.task_id, &left.outcome_key).cmp(&(
                &right.instance_id,
                &right.task_id,
                &right.outcome_key,
            ))
        });
        projected.tasks.sort_by(|left, right| {
            (&left.instance_id, &left.task_id).cmp(&(&right.instance_id, &right.task_id))
        });
        projected.instances.iter_mut().for_each(|instance| {
            instance.capability_operation_ids.sort();
            instance.preferred_task_ids.sort();
        });
        projected
            .instances
            .sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
        let authority_revisions = selected
            .iter()
            .map(|((scope, key), stored)| {
                (
                    scope,
                    key,
                    &stored.record.source_snapshot_id,
                    stored.sequence,
                    stored.event_id,
                )
            })
            .collect::<Vec<_>>();
        projected.ledger_position = ledger_position;
        projected.fact_snapshot_id =
            combined_policy_snapshot_id(&projected, resources, &authority_revisions)?;
        Ok(projected)
    }

    #[cfg(test)]
    fn active_count(&self) -> usize {
        self.active.len()
    }
}

fn fact_tombstone_key(
    scope: &FactScope,
    key: &str,
    source_snapshot_id: &str,
) -> RuntimeHostResult<String> {
    let identity = serde_json::to_vec(&(scope, key, source_snapshot_id)).map_err(|_| {
        fact_fatal(
            "fact_tombstone_identity_encode_failed",
            "identify_fact_tombstone",
        )
    })?;
    Ok(format!("{:x}", Sha256::digest(identity)))
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

fn combined_policy_snapshot_id<T: serde::Serialize>(
    facts: &EvaluationFacts,
    resources: &EvaluationResources,
    authority_revisions: &[T],
) -> RuntimeHostResult<String> {
    let mut resources = resources.clone();
    resources
        .pools
        .sort_by(|left, right| left.pool_id.cmp(&right.pool_id));
    resources
        .hosts
        .sort_by(|left, right| left.host_id.cmp(&right.host_id));
    let bytes = serde_json::to_vec(&(
        &facts.facts,
        &facts.outcomes,
        &facts.tasks,
        &facts.instances,
        &resources,
        authority_revisions,
    ))
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
    use actingcommand_contract::{FactTtlPolicy, FactTtlSource};
    use actingcommand_policy::{
        HostResourceSnapshot, ObservedOutcome, PoolValueSnapshot, TaskRuntimeSnapshot,
    };
    use tempfile::TempDir;

    fn store_with_state(state: Arc<RuntimeStateStore>) -> InstanceFactStore {
        InstanceFactStore {
            active: BTreeMap::new(),
            invalidated: BTreeMap::new(),
            pending: BTreeMap::new(),
            last_sequence: 0,
            state,
        }
    }

    fn empty_store() -> (TempDir, InstanceFactStore) {
        let root = TempDir::new().expect("tempdir");
        let state = Arc::new(
            RuntimeStateStore::open(root.path(), b"0123456789abcdef").expect("state store"),
        );
        (root, store_with_state(state))
    }

    fn resources() -> EvaluationResources {
        EvaluationResources {
            pools: vec![PoolValueSnapshot {
                pool_id: "pool-a".to_owned(),
                value: 10,
                observed_at_unix_ms: 1_000,
            }],
            hosts: vec![HostResourceSnapshot {
                host_id: "host-a".to_owned(),
                cpu_available_milli: 1_000,
                gpu_available_milli: 1_000,
                io_available_milli: 1_000,
                host_responsiveness_basis_points: 10_000,
                third_party_pressure_basis_points: 0,
                heavy_dispatch_limit: 1,
                active_heavy_dispatches: 0,
            }],
        }
    }

    fn record(scope: FactScope, snapshot: &str, invalidate_on: Vec<EventType>) -> FactRecord {
        FactRecord {
            scope,
            key: "env.theme".to_owned(),
            content: FactContent::Inline {
                value: ContractFactValue::String("Neutral".to_owned()),
            },
            observed_at_unix_ms: 1_000,
            expires_at_unix_ms: Some(5_000),
            ttl_policy: Some(FactTtlPolicy {
                minimum_ms: 1_000,
                maximum_ms: 10_000,
                source: FactTtlSource::DetectorContract,
            }),
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
        let (_root, mut store) = empty_store();
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
        let (_root, mut store) = empty_store();
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

    #[test]
    fn policy_snapshot_identity_covers_every_evaluator_fact_dimension() {
        let (_root, store) = empty_store();
        let base = EvaluationFacts {
            ledger_position: 1,
            fact_snapshot_id: "caller-snapshot-a".to_owned(),
            facts: vec![ObservedFact {
                scope: ScopeSelector::Instance {
                    instance_id: "instance-a".to_owned(),
                },
                fact_key: "env.theme".to_owned(),
                value: PolicyFactValue::String("Neutral".to_owned()),
                observed_at_unix_ms: 1_000,
                expires_at_unix_ms: Some(2_000),
                confidence_milli: 900,
            }],
            outcomes: vec![ObservedOutcome {
                task_id: "task-a".to_owned(),
                instance_id: "instance-a".to_owned(),
                outcome_key: "completed".to_owned(),
                value: PolicyFactValue::Boolean(false),
                observed_at_unix_ms: 1_000,
            }],
            tasks: vec![TaskRuntimeSnapshot {
                task_id: "task-a".to_owned(),
                instance_id: "instance-a".to_owned(),
                last_dispatched_unix_ms: None,
                eligible_since_unix_ms: Some(1_000),
                terminal_state: None,
            }],
            instances: vec![InstanceSnapshot {
                instance_id: "instance-a".to_owned(),
                server_id: "server-a".to_owned(),
                game_id: "game-a".to_owned(),
                host_id: "host-a".to_owned(),
                available: true,
                capability_operation_ids: vec!["operation-a".to_owned()],
                preferred_task_ids: vec!["task-a".to_owned()],
            }],
        };
        let resources = resources();
        let canonical = store
            .overlay_policy_facts(&base, &resources, 7)
            .expect("canonical facts");

        let mut caller_identity_changed = base.clone();
        caller_identity_changed.fact_snapshot_id = "caller-snapshot-b".to_owned();
        assert_eq!(
            store
                .overlay_policy_facts(&caller_identity_changed, &resources, 99)
                .expect("canonical caller-independent facts")
                .fact_snapshot_id,
            canonical.fact_snapshot_id
        );

        let mut variants = Vec::new();
        let mut changed = base.clone();
        changed.facts[0].confidence_milli = 899;
        variants.push(changed);
        let mut changed = base.clone();
        changed.outcomes[0].value = PolicyFactValue::Boolean(true);
        variants.push(changed);
        let mut changed = base.clone();
        changed.tasks[0].eligible_since_unix_ms = Some(999);
        variants.push(changed);
        let mut changed = base.clone();
        changed.instances[0].server_id = "server-b".to_owned();
        variants.push(changed);
        let mut changed = base.clone();
        changed.instances[0].game_id = "game-b".to_owned();
        variants.push(changed);

        for changed in variants {
            assert_ne!(
                store
                    .overlay_policy_facts(&changed, &resources, 7)
                    .expect("changed facts")
                    .fact_snapshot_id,
                canonical.fact_snapshot_id
            );
        }

        let mut changed_resources = resources.clone();
        changed_resources.pools[0].value += 1;
        assert_ne!(
            store
                .overlay_policy_facts(&base, &changed_resources, 7)
                .expect("changed resources")
                .fact_snapshot_id,
            canonical.fact_snapshot_id
        );
    }

    #[test]
    fn fact_tombstones_compact_in_memory_without_losing_durable_rejection() {
        let root = TempDir::new().expect("tempdir");
        let state = Arc::new(
            RuntimeStateStore::open(root.path(), b"0123456789abcdef").expect("state store"),
        );
        let mut store = store_with_state(Arc::clone(&state));
        let issuer = actingcommand_contract::IdentifierIssuer::new().expect("issuer");
        let scope = FactScope::Instance {
            instance_id: "instance-a".to_owned(),
        };
        let mut first = None;
        let mut sequence = 1_u64;

        for index in 0..(MAX_RECENT_FACT_TOMBSTONES + 32) {
            let snapshot = format!("snapshot:compaction-{index}");
            let fact = record(scope.clone(), &snapshot, vec![EventType::RuntimeTakeover]);
            first.get_or_insert_with(|| fact.clone());
            store
                .commit_publish(
                    fact,
                    sequence,
                    *issuer.mint_event_id().expect("publish event").transport(),
                )
                .expect("publish fact");
            sequence += 1;
            store
                .commit_invalidation(
                    FactInvalidationEventData {
                        scope: scope.clone(),
                        key: "env.theme".to_owned(),
                        source_snapshot_id: snapshot,
                        invalidated_at_unix_ms: 2_000 + index as u64,
                        invalidated_by_event_id: *issuer
                            .mint_event_id()
                            .expect("invalidation event")
                            .transport(),
                        invalidated_by_event_type: EventType::RuntimeTakeover,
                    },
                    sequence,
                )
                .expect("invalidate fact");
            sequence += 1;
        }

        assert_eq!(store.invalidated.len(), MAX_RECENT_FACT_TOMBSTONES);
        assert_eq!(store.active_count(), 0);
        let first = first.expect("first fact");
        assert_eq!(
            store
                .preview_publish(&first)
                .expect_err("compacted tombstone must still reject republish")
                .code(),
            "fact_source_snapshot_invalidated"
        );

        drop(store);
        let recovered = store_with_state(state);
        assert_eq!(
            recovered
                .preview_publish(&first)
                .expect_err("durable tombstone must survive projection reconstruction")
                .code(),
            "fact_source_snapshot_invalidated"
        );
    }
}
