// SPDX-License-Identifier: AGPL-3.0-only

//! The instance fact store (Workflow #383 AO-3, `contracts/ledger-observer.md` §1, §3): the
//! live memory of the observations and instructions, applied first by its one entry and copied to
//! the ledger by the gates. The start still rebuilds it from the ledger (AO-6 retires that), and
//! until business opens it follows the ledger tail as before; from then on it reads no ledger:
//! every change is applied here, counted in its lineage's revision, and every recorded event
//! reaches [`InstanceFactStore::observe_trigger`] at capture.

use crate::codes::{HostCode, HostLocation};
use crate::recovery_read::{RECOVERY_PAGE_EVENTS, read_pages};
use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::codes::{ContractLocation, SharedCode};
use actingcommand_contract::outcome::{Code, Location};
use actingcommand_contract::{
    EventActor, EventId, EventPayload, EventQuery, EventType, FactContent,
    FactInvalidationEventData, FactPayload, FactRecord, FactScalar as ContractFactScalar,
    FactScope, FactValue as ContractFactValue, InstanceFactContext, InstanceFactSnapshot,
    OwnerEpoch, RESOURCE_TARGETS_FACT_KEY, RuntimeErrorCode, RuntimeStateStoreRevision,
    priority_offset_task_id,
};
use actingcommand_ledger::{GlobalLedger, PersistedEvent};
use actingcommand_policy::{
    EvaluationFacts, EvaluationResources, FactScalar as PolicyFactScalar,
    FactValue as PolicyFactValue, InstanceSnapshot, MAX_ID_BYTES, MAX_PRIORITY_OFFSET_MILLI,
    ObservedFact, PriorityOffsetOrigin, ScopeSelector,
};
use actingcommand_runtime_state::{
    FACT_TOMBSTONE_NAMESPACE, RuntimeStateStore, fact_tombstone_key,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

type FactIdentity = (FactScope, String);
type InvalidationIdentity = (FactScope, String, String, EventId);
type HistoricalInvalidationIdentity = (FactIdentity, String);

/// Workflow #383 AO-3 (`contracts/ledger-observer.md` §3, §8.1): where a record or an event
/// stands in the store's history. What the start replayed from the ledger (everything committed
/// before business opened) stands at its ledger sequence; everything of the live lineage stands
/// at its capture number in this epoch, after everything replayed. Capture order is ledger order
/// within an epoch (§8.1), so the two compare as the ledger orders them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum FactOrder {
    Replayed(u64),
    Captured(u64),
}

impl FactOrder {
    /// A live event's place: its capture number in this epoch.
    pub(crate) fn of_live_event(event: &PersistedEvent) -> Option<Self> {
        event
            .capture()
            .map(|capture| Self::Captured(capture.capture_seq))
    }
}

/// Workflow #383 AO-3 (§3): one recorded event as [`InstanceFactStore::observe_trigger`] sees it:
/// its type, id, instance link and time, and, for a record observed after its commit, its place
/// in the store's history (a record observed at its own capture comes after everything the store
/// holds).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FactTrigger {
    pub(crate) event_type: EventType,
    pub(crate) event_id: EventId,
    pub(crate) instance_id: Option<actingcommand_contract::InstanceId>,
    pub(crate) at_unix_ms: u64,
    pub(crate) order: Option<FactOrder>,
}

impl FactTrigger {
    /// A record observed right after its commit, at its capture number; a record the ledger
    /// drafted itself has none and comes after everything the store holds.
    pub(crate) fn committed(event: &PersistedEvent) -> Self {
        Self {
            event_type: event.event_type(),
            event_id: *event.event_id(),
            instance_id: event.links().instance_id().copied(),
            at_unix_ms: event.timestamp_unix_ms(),
            order: FactOrder::of_live_event(event),
        }
    }

    /// A record observed at its own capture, from its sanitized draft.
    pub(crate) fn at_capture(draft: &actingcommand_contract::SanitizedEventDraft) -> Self {
        Self {
            event_type: draft.event_type(),
            event_id: *draft.event_id(),
            instance_id: draft.links().instance_id().copied(),
            at_unix_ms: draft.timestamp_unix_ms(),
            order: None,
        }
    }
}

/// Workflow #383 AO-3 (§3): an invalidation the store applied for a trigger, with the revision
/// it took; the gate copies it after its trigger.
#[derive(Debug, Clone)]
pub(crate) struct AppliedInvalidation {
    pub(crate) data: FactInvalidationEventData,
    pub(crate) revision: u64,
}

/// Replay metadata, retained even when no fact is active for an instance.
#[derive(Clone, Default)]
struct InputInvalidationBoundaries {
    by_instance: BTreeMap<actingcommand_contract::InstanceId, [u64; 2]>,
}

impl InputInvalidationBoundaries {
    fn observe(&mut self, event: &PersistedEvent) -> RuntimeHostResult<()> {
        self.observe_type(
            event.event_type(),
            event.links().instance_id(),
            event.timestamp_unix_ms(),
        )
    }

    fn observe_type(
        &mut self,
        event_type: EventType,
        instance: Option<&actingcommand_contract::InstanceId>,
        at_unix_ms: u64,
    ) -> RuntimeHostResult<()> {
        let index = match event_type {
            EventType::InputCommitted => 0,
            EventType::InputFailed => 1,
            _ => return Ok(()),
        };
        let instance = instance.ok_or_else(|| {
            fact_fatal(
                HostCode::FactInputScopeMissing,
                HostLocation::ReplayFactInputBoundary,
            )
        })?;
        if !self.by_instance.contains_key(instance)
            && self.by_instance.len() >= actingcommand_policy::MAX_EVALUATION_INSTANCES
        {
            return Err(fact_fatal(
                HostCode::FactInputScopeCapacityExceeded,
                HostLocation::ReplayFactInputBoundary,
            ));
        }
        let boundary = self.by_instance.entry(*instance).or_default();
        boundary[index] = boundary[index].max(at_unix_ms);
        Ok(())
    }

    fn check(
        &self,
        record: &FactRecord,
        instances: &[actingcommand_contract::InstanceId],
    ) -> RuntimeHostResult<()> {
        for instance in instances {
            if let Some(boundaries) = self.by_instance.get(instance) {
                for (index, event) in [EventType::InputCommitted, EventType::InputFailed]
                    .iter()
                    .enumerate()
                {
                    if record.invalidate_on.contains(event)
                        && record.observed_at_unix_ms <= boundaries[index]
                    {
                        return Err(fact_request(
                            HostCode::FactObservationPrecedesInput,
                            ContractLocation::PublishFacts,
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

fn invalidation_scope_matches(
    event: &PersistedEvent,
    instances: &[actingcommand_contract::InstanceId],
) -> bool {
    trigger_scope_matches(event.event_type(), event.links().instance_id(), instances)
}

fn trigger_scope_matches(
    event_type: EventType,
    instance: Option<&actingcommand_contract::InstanceId>,
    instances: &[actingcommand_contract::InstanceId],
) -> bool {
    match instance {
        Some(instance) => {
            instances.contains(instance)
                || (instances.is_empty()
                    && !matches!(
                        event_type,
                        EventType::InputCommitted | EventType::InputFailed
                    ))
        }
        None => !matches!(
            event_type,
            EventType::InputCommitted | EventType::InputFailed
        ),
    }
}
const MAX_ACTIVE_FACTS: usize = 256;

/// The policy instance facts (Workflow #313 item 4): the evaluation's `instances` read these
/// three keys from the fact store for every configured instance; the configuration only seeds
/// them at startup. The instance projection consumes them, so they are never overlaid as
/// ordinary policy facts.
pub(crate) const POLICY_INSTANCE_AVAILABLE_KEY: &str = "session.instance.available";
pub(crate) const POLICY_INSTANCE_CAPABILITIES_KEY: &str = "session.instance.capabilities";
pub(crate) const POLICY_INSTANCE_PREFERRED_TASKS_KEY: &str = "session.instance.preferred_tasks";
/// Row field of the `record_list` value of the two list-typed policy instance facts.
pub(crate) const POLICY_INSTANCE_OPERATION_FIELD: &str = "operation_id";
pub(crate) const POLICY_INSTANCE_TASK_FIELD: &str = "task_id";

pub(crate) fn is_policy_instance_fact_key(key: &str) -> bool {
    key == POLICY_INSTANCE_AVAILABLE_KEY
        || key == POLICY_INSTANCE_CAPABILITIES_KEY
        || key == POLICY_INSTANCE_PREFERRED_TASKS_KEY
}

/// The task identifier charset of the scheduling schema (`^[a-z0-9][a-z0-9._:-]*$`, 1 to
/// 128 bytes), required of the `<task_id>` of a priority offset key.
pub(crate) fn valid_priority_offset_task_id(task_id: &str) -> bool {
    !task_id.is_empty()
        && task_id.len() <= MAX_ID_BYTES
        && task_id.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || (index > 0 && matches!(byte, b'.' | b'_' | b':' | b'-'))
        })
}

/// The offset in milli a priority offset record carries: an inline integer within
/// `±MAX_PRIORITY_OFFSET_MILLI`; `None` for any other content.
pub(crate) fn priority_offset_milli(record: &FactRecord) -> Option<i32> {
    let FactContent::Inline {
        value: ContractFactValue::Integer(value),
    } = &record.content
    else {
        return None;
    };
    i32::try_from(*value)
        .ok()
        .filter(|value| (-MAX_PRIORITY_OFFSET_MILLI..=MAX_PRIORITY_OFFSET_MILLI).contains(value))
}

/// One active `session.task.<task_id>.priority_offset` record that applies to at least one
/// evaluated instance (Workflow #308 slice 4a-2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActivePriorityOffset {
    pub(crate) scope: FactScope,
    pub(crate) task_id: String,
    pub(crate) offset_milli: i32,
    pub(crate) origin: PriorityOffsetOrigin,
    pub(crate) observed_at_unix_ms: u64,
    pub(crate) expires_at_unix_ms: Option<u64>,
}

/// A person (the console's User or the operator's Cli) published the offset; any other
/// publishing actor — an agent, or the Runtime itself for a record published before offsets
/// carried their request's origin — is an agent.
fn priority_offset_origin(publisher: EventActor) -> PriorityOffsetOrigin {
    match publisher {
        EventActor::User | EventActor::Cli => PriorityOffsetOrigin::User,
        _ => PriorityOffsetOrigin::Agent,
    }
}

/// Workflow #390 POOL-1: a record the Runtime itself published from a package run, a resource
/// reading (`resource_reading:`) or an adjustment of one (`resource_adjustment:`). It carries no
/// input invalidation (`contracts/resource-readings.md`); a `ledger_fact` pool takes it as its
/// base, which the next reading or adjustment of the key resets.
fn package_reading(stored: &StoredFact) -> bool {
    stored.publisher == EventActor::Runtime
        && [
            actingcommand_contract::RESOURCE_READING_DETECTOR_PREFIX,
            actingcommand_contract::RESOURCE_ADJUSTMENT_DETECTOR_PREFIX,
        ]
        .iter()
        .any(|prefix| stored.record.source_detector.starts_with(prefix))
}

fn scope_specificity(scope: &FactScope) -> u8 {
    match scope {
        FactScope::Instance { .. } => 3,
        FactScope::Server { .. } => 2,
        FactScope::Game { .. } => 1,
    }
}
const MAX_RECENT_FACT_TOMBSTONES: usize = 256;

#[derive(Clone, Copy)]
enum PolicyFactConflictDisposition {
    FatalInvariant,
    RejectCaller,
}

#[derive(Clone)]
struct StoredFact {
    record: FactRecord,
    /// Workflow #383 AO-3: where its `fact.published` stands in the store's history.
    order: FactOrder,
    /// The ledger sequence of its `fact.published` when the store knows it: a record the start
    /// replayed, or a publication whose capture waited for its commit.
    sequence: Option<u64>,
    event_id: EventId,
    scope_instances: Vec<actingcommand_contract::InstanceId>,
    /// The actor of the `fact.published` event that carried the record.
    publisher: EventActor,
}

#[derive(Clone)]
struct InvalidationTombstone {
    data: FactInvalidationEventData,
    /// Where its `fact.invalidated` stands; a live invalidation stands at its trigger until its
    /// own record is captured.
    order: FactOrder,
}

/// An invalidation the start replay derived and the start has not written yet (until business
/// opens, the store follows the ledger tail; §3 applies from then on).
#[derive(Clone)]
pub(crate) struct PendingFactInvalidation {
    pub(crate) data: FactInvalidationEventData,
    /// The start wrote it; its record settles it once the store follows the ledger past it.
    written: bool,
}

/// The active record of a key as [`InstanceFactStore::active_revision`] reads it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ActiveFact<'a> {
    pub(crate) record: &'a FactRecord,
    pub(crate) order: FactOrder,
    pub(crate) sequence: Option<u64>,
    pub(crate) event_id: EventId,
}

/// Workflow #383 AO-3 (§3): until business opens the store follows the ledger tail as before;
/// from then on it is live: it reads no ledger, counts every accepted change in its lineage and
/// observes every recorded event at capture.
#[derive(Clone, Copy, PartialEq, Eq)]
enum StoreMode {
    Replaying,
    Live,
}

/// A capture number not issued yet: the order of a record between its application and its
/// capture, both under `fact_write_gate`.
const UNSETTLED_ORDER: FactOrder = FactOrder::Captured(u64::MAX);

fn fact_state_error(error: actingcommand_runtime_state::RuntimeStateError) -> RuntimeHostError {
    RuntimeHostError::state(&error).with_native_detail(error.to_string())
}

pub(crate) fn fact_transaction_error(
    error: actingcommand_ledger::GlobalLedgerError,
) -> RuntimeHostError {
    if let Some(work) = error.rolled_back_work() {
        let mapped = if work.fatal {
            RuntimeHostError::relayed_fatal(
                work.code,
                work.operation,
                RuntimeErrorCode::RuntimeHalted,
            )
        } else {
            RuntimeHostError::relayed_request(
                work.code,
                work.operation,
                RuntimeErrorCode::InvalidRequest,
            )
        };
        mapped.with_native_detail(work.detail.clone())
    } else {
        RuntimeHostError::relayed_fatal(
            error.code(),
            error.operation(),
            RuntimeErrorCode::LedgerFailure,
        )
        .with_native_detail(format!("{error}; detail={:?}", error.detail()))
    }
}

fn original_at(ledger: &GlobalLedger, sequence: u64) -> RuntimeHostResult<PersistedEvent> {
    let mut events = ledger
        .query(EventQuery {
            from_sequence: Some(sequence),
            to_sequence: Some(sequence),
            ..EventQuery::default()
        })
        .map_err(fact_transaction_error)?;
    if events.len() != 1 {
        return Err(fact_fatal(
            HostCode::FactProjectionSourceMissing,
            HostLocation::ReadFactSource,
        ));
    }
    Ok(events.remove(0))
}

#[derive(Default)]
struct HistoricalFactProjection {
    active: BTreeMap<FactIdentity, (FactRecord, u64, EventId)>,
    scope_instances: BTreeMap<FactIdentity, Vec<actingcommand_contract::InstanceId>>,
    publishers: BTreeMap<FactIdentity, EventActor>,
    invalidated: BTreeMap<HistoricalInvalidationIdentity, FactInvalidationEventData>,
    input_boundaries: InputInvalidationBoundaries,
}

impl HistoricalFactProjection {
    fn replay(&mut self, event: &PersistedEvent) -> RuntimeHostResult<()> {
        match event.payload() {
            EventPayload::Fact(FactPayload::Published(payload)) => {
                self.input_boundaries
                    .check(payload.record(), payload.scope_instances())
                    .map_err(|_| {
                        fact_fatal(
                            HostCode::FactObservationPrecedesInput,
                            HostLocation::ProjectFactHistory,
                        )
                    })?;
                for record in payload.records() {
                    self.publish(record.clone(), event.sequence(), *event.event_id())?;
                    self.scope_instances.insert(
                        (record.scope.clone(), record.key.clone()),
                        payload.scope_instances().to_vec(),
                    );
                    self.publishers.insert(
                        (record.scope.clone(), record.key.clone()),
                        event.origin().actor(),
                    );
                }
                Ok(())
            }
            EventPayload::Fact(FactPayload::Invalidated(payload)) => {
                let data = payload.invalidation();
                let identity = (
                    (data.scope.clone(), data.key.clone()),
                    data.source_snapshot_id.clone(),
                );
                // Workflow #384 D2-S3: an adjustment's invalidation is derived from no trigger
                // event; it applies where it stands.
                if !self.invalidated.contains_key(&identity)
                    && actingcommand_contract::is_adjustment_invalidation(data)
                {
                    return self.invalidate(data.clone());
                }
                if self.invalidated.get(&identity) != Some(data) {
                    return Err(fact_fatal(
                        SharedCode::FactInvalidationSourceMismatch,
                        HostLocation::ProjectFactHistory,
                    ));
                }
                Ok(())
            }
            _ => {
                self.input_boundaries.observe(event)?;
                let invalidations = self
                    .active
                    .values()
                    .filter(|(record, _, _)| {
                        record.invalidate_on.contains(&event.event_type())
                            && invalidation_scope_matches(
                                event,
                                self.scope_instances
                                    .get(&(record.scope.clone(), record.key.clone()))
                                    .map_or(&[], Vec::as_slice),
                            )
                    })
                    .map(|(record, _, _)| FactInvalidationEventData {
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

    fn publish(
        &mut self,
        record: FactRecord,
        sequence: u64,
        event_id: EventId,
    ) -> RuntimeHostResult<()> {
        record.validate().map_err(|_| {
            fact_fatal(
                HostCode::FactRecordInvalid,
                HostLocation::ProjectFactHistory,
            )
        })?;
        let identity = (record.scope.clone(), record.key.clone());
        if self
            .invalidated
            .contains_key(&(identity.clone(), record.source_snapshot_id.clone()))
        {
            return Err(fact_fatal(
                HostCode::FactSourceSnapshotRepublished,
                HostLocation::ProjectFactHistory,
            ));
        }
        if let Some((existing, _, existing_event_id)) = self.active.get(&identity)
            && existing.source_snapshot_id == record.source_snapshot_id
        {
            if existing != &record || existing_event_id != &event_id {
                return Err(fact_fatal(
                    HostCode::FactSourceSnapshotIdentityConflict,
                    HostLocation::ProjectFactHistory,
                ));
            }
            return Ok(());
        }
        if !self.active.contains_key(&identity) && self.active.len() >= MAX_ACTIVE_FACTS {
            return Err(fact_fatal(
                HostCode::FactStoreCapacityExceeded,
                HostLocation::ProjectFactHistory,
            ));
        }
        self.active.insert(identity, (record, sequence, event_id));
        Ok(())
    }

    fn invalidate(&mut self, data: FactInvalidationEventData) -> RuntimeHostResult<()> {
        let identity = (data.scope.clone(), data.key.clone());
        let invalidation_identity = (identity.clone(), data.source_snapshot_id.clone());
        if let Some(existing) = self.invalidated.get(&invalidation_identity) {
            if existing != &data {
                return Err(fact_fatal(
                    SharedCode::FactInvalidationIdentityConflict,
                    HostLocation::ProjectFactHistory,
                ));
            }
            return Ok(());
        }
        let (active, _, _) = self.active.get(&identity).ok_or_else(|| {
            fact_fatal(
                HostCode::FactInvalidationTargetMissing,
                HostLocation::ProjectFactHistory,
            )
        })?;
        if active.source_snapshot_id != data.source_snapshot_id
            || !(active
                .invalidate_on
                .contains(&data.invalidated_by_event_type)
                || actingcommand_contract::is_adjustment_invalidation(&data))
        {
            return Err(fact_fatal(
                SharedCode::FactInvalidationTargetMismatch,
                HostLocation::ProjectFactHistory,
            ));
        }
        self.active.remove(&identity);
        self.invalidated.insert(invalidation_identity, data);
        self.scope_instances.remove(&identity);
        self.publishers.remove(&identity);
        Ok(())
    }

    fn records(self) -> Vec<FactRecord> {
        self.active
            .into_values()
            .map(|(record, _, _)| record)
            .collect()
    }
}

#[derive(Clone)]
pub(crate) struct InstanceFactStore {
    active: BTreeMap<FactIdentity, StoredFact>,
    latest_observed: BTreeMap<FactIdentity, u64>,
    input_boundaries: InputInvalidationBoundaries,
    invalidated: BTreeMap<(FactIdentity, String), InvalidationTombstone>,
    pending: BTreeMap<InvalidationIdentity, PendingFactInvalidation>,
    /// The ledger position the store has followed through while replaying.
    last_sequence: u64,
    mode: StoreMode,
    /// Workflow #383 AO-3 (§8.3): the store's lineage (the owner epoch) and its revision, +1 per
    /// accepted change once it is live; 0 is the state it went live with.
    lineage: OwnerEpoch,
    revision: u64,
    state: Arc<RuntimeStateStore>,
}

impl InstanceFactStore {
    /// The live store's active records, only when it has replayed exactly through
    /// `ledger_position` (Workflow #317 item E); `None` sends the caller to
    /// `active_records_at`, which replays the ledger for any other position. A live store
    /// follows no ledger position (Workflow #383 AO-3).
    pub(crate) fn active_records_if_at(&self, ledger_position: u64) -> Option<Vec<FactRecord>> {
        (self.mode == StoreMode::Replaying
            && ledger_position != 0
            && ledger_position == self.last_sequence)
            .then(|| {
                self.active
                    .values()
                    .map(|stored| stored.record.clone())
                    .collect()
            })
    }

    pub(crate) fn active_records_at(
        ledger: &GlobalLedger,
        ledger_position: u64,
    ) -> RuntimeHostResult<Vec<FactRecord>> {
        let latest = ledger.latest_sequence().map_err(|_| {
            fact_fatal(
                HostCode::FactStoreQueryFailed,
                HostLocation::ProjectFactHistory,
            )
        })?;
        if ledger_position == 0 || ledger_position > latest {
            return Err(fact_fatal(
                HostCode::FactLedgerPositionInvalid,
                HostLocation::ProjectFactHistory,
            ));
        }
        let events = ledger
            .query(EventQuery {
                to_sequence: Some(ledger_position),
                ..EventQuery::default()
            })
            .map_err(|_| {
                fact_fatal(
                    HostCode::FactStoreQueryFailed,
                    HostLocation::ProjectFactHistory,
                )
            })?;
        let mut projection = HistoricalFactProjection::default();
        for event in events {
            projection.replay(&event)?;
        }
        Ok(projection.records())
    }

    /// Rebuilds the store from the whole ledger (the start's replay, until AO-6). The store
    /// replays until [`Self::go_live`]; `lineage` is this start's owner epoch.
    pub(crate) fn recover(
        ledger: &GlobalLedger,
        state: Arc<RuntimeStateStore>,
        lineage: OwnerEpoch,
    ) -> RuntimeHostResult<Self> {
        let mut store = Self {
            active: BTreeMap::new(),
            latest_observed: BTreeMap::new(),
            input_boundaries: InputInvalidationBoundaries::default(),
            invalidated: BTreeMap::new(),
            pending: BTreeMap::new(),
            last_sequence: 0,
            mode: StoreMode::Replaying,
            lineage,
            revision: 0,
            state,
        };
        // Workflow #381 R4a: the whole Ledger through one position, in bounded pages.
        let events = ledger
            .latest_sequence()
            .and_then(|through| {
                read_pages(
                    ledger,
                    &EventQuery::default(),
                    through,
                    RECOVERY_PAGE_EVENTS,
                )
            })
            .map_err(|_| {
                fact_fatal(
                    HostCode::FactStoreRecoveryFailed,
                    HostLocation::RecoverFactStore,
                )
            })?;
        let mut history = HistoricalFactProjection::default();
        for event in &events {
            history.replay(event)?;
        }
        store
            .state
            .recover_fact_projections(&events)
            .map_err(fact_state_error)?;
        for event in events {
            store.replay_event(&event, ledger)?;
        }
        Ok(store)
    }

    /// Follows the ledger tail while the store replays (the start, until business opens). A live
    /// store has observed every recorded event at its capture and reads no ledger (Workflow #383
    /// AO-3, §3); for it there is nothing to follow.
    pub(crate) fn synchronize(&mut self, ledger: &GlobalLedger) -> RuntimeHostResult<()> {
        if self.mode == StoreMode::Live {
            return Ok(());
        }
        let from_sequence = self.last_sequence.checked_add(1).ok_or_else(|| {
            fact_fatal(
                HostCode::FactSequenceOverflow,
                HostLocation::SynchronizeFactStore,
            )
        })?;
        let events = ledger
            .query(EventQuery {
                from_sequence: Some(from_sequence),
                ..EventQuery::default()
            })
            .map_err(|_| {
                fact_fatal(
                    HostCode::FactStoreSyncFailed,
                    HostLocation::SynchronizeFactStore,
                )
            })?;
        for event in events {
            self.replay_event(&event, ledger)?;
        }
        Ok(())
    }

    /// The invalidations the replay derived and the start has not written yet.
    pub(crate) fn pending_invalidations(&self) -> Vec<PendingFactInvalidation> {
        self.pending
            .values()
            .filter(|pending| !pending.written)
            .cloned()
            .collect()
    }

    /// Whether an invalidation the replay derived is still unwritten or unsettled.
    pub(crate) fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Workflow #383 AO-3: whether the store is live (§3 applies).
    pub(crate) fn is_live(&self) -> bool {
        self.mode == StoreMode::Live
    }

    /// Workflow #383 AO-3 (§3, §8.3): the store stops following the ledger and goes live with the
    /// state it holds, at revision 0 of its lineage. Every replayed invalidation must be written
    /// by then.
    pub(crate) fn go_live(&mut self) -> RuntimeHostResult<()> {
        if self.mode == StoreMode::Live || !self.pending.is_empty() {
            return Err(fact_fatal(
                HostCode::FactStoreNotLive,
                HostLocation::SynchronizeFactStore,
            ));
        }
        self.mode = StoreMode::Live;
        self.revision = 0;
        Ok(())
    }

    /// Workflow #383 AO-3 (§13, register W1): the store's lineage and its revision.
    pub(crate) fn revision(&self) -> RuntimeStateStoreRevision {
        RuntimeStateStoreRevision {
            lineage: self.lineage,
            revision: self.revision,
        }
    }

    fn require_live(&self, operation: impl Into<Location>) -> RuntimeHostResult<()> {
        if self.mode != StoreMode::Live {
            return Err(fact_fatal(HostCode::FactStoreNotLive, operation));
        }
        Ok(())
    }

    fn next_revision(&mut self, operation: impl Into<Location>) -> RuntimeHostResult<u64> {
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| fact_fatal(HostCode::FactSequenceOverflow, operation))?;
        Ok(self.revision)
    }

    /// Workflow #383 AO-3, the store's one entry for a publication (G2): applies the records of
    /// one `fact.published` (`event_id`), checked first by [`Self::preview_observation`], and
    /// returns the revision the change took. The records stand unsettled until the gate has
    /// captured them ([`Self::settle_publication`]).
    pub(crate) fn apply_publication(
        &mut self,
        records: &[FactRecord],
        scope_instances: &[actingcommand_contract::InstanceId],
        publisher: EventActor,
        event_id: EventId,
    ) -> RuntimeHostResult<u64> {
        self.require_live(HostLocation::CommitFact)?;
        let mut applied = self.clone();
        for record in records {
            applied.commit_publish(record.clone(), UNSETTLED_ORDER, None, event_id)?;
            let stored = applied
                .active
                .get_mut(&(record.scope.clone(), record.key.clone()))
                .ok_or_else(|| fact_fatal(HostCode::FactCommitMissing, HostLocation::CommitFact))?;
            stored.scope_instances = scope_instances.to_vec();
            stored.publisher = publisher;
        }
        let revision = applied.next_revision(HostLocation::CommitFact)?;
        *self = applied;
        Ok(revision)
    }

    /// Workflow #383 AO-3: the place of a publication's records once the gate captured their
    /// `fact.published`, and its ledger sequence when the capture waited for the commit.
    pub(crate) fn settle_publication(
        &mut self,
        event_id: EventId,
        order: FactOrder,
        sequence: Option<u64>,
    ) {
        for stored in self
            .active
            .values_mut()
            .filter(|stored| stored.event_id == event_id)
        {
            stored.order = order;
            stored.sequence = sequence;
        }
    }

    /// Workflow #383 AO-3, `contracts/ledger-observer.md` §3: one recorded event, at its capture
    /// (or, for a record the ledger drafted itself, right after it): records the input boundary,
    /// plans the `invalidate_on` invalidations and applies them at once, each one change of the
    /// store. Returns them in application order, for the gate to capture after the event. A
    /// record observed after its commit invalidates only records that stand before it.
    /// `fact.published` and `fact.invalidated` never trigger (§3).
    pub(crate) fn observe_trigger(
        &mut self,
        trigger: &FactTrigger,
    ) -> RuntimeHostResult<Vec<AppliedInvalidation>> {
        self.require_live(HostLocation::DeriveFactInvalidation)?;
        if matches!(
            trigger.event_type,
            EventType::FactPublished | EventType::FactInvalidated
        ) {
            return Ok(Vec::new());
        }
        self.input_boundaries.observe_type(
            trigger.event_type,
            trigger.instance_id.as_ref(),
            trigger.at_unix_ms,
        )?;
        let planned = self
            .active
            .values()
            .filter(|stored| {
                trigger.order.is_none_or(|order| stored.order < order)
                    && stored.record.invalidate_on.contains(&trigger.event_type)
                    && trigger_scope_matches(
                        trigger.event_type,
                        trigger.instance_id.as_ref(),
                        &stored.scope_instances,
                    )
            })
            .map(|stored| FactInvalidationEventData {
                scope: stored.record.scope.clone(),
                key: stored.record.key.clone(),
                source_snapshot_id: stored.record.source_snapshot_id.clone(),
                invalidated_at_unix_ms: trigger.at_unix_ms,
                invalidated_by_event_id: trigger.event_id,
                invalidated_by_event_type: trigger.event_type,
            })
            .collect::<Vec<_>>();
        let mut applied = Vec::with_capacity(planned.len());
        for data in planned {
            self.apply_invalidation(data.clone(), UNSETTLED_ORDER, true)?;
            let revision = self.next_revision(HostLocation::DeriveFactInvalidation)?;
            applied.push(AppliedInvalidation { data, revision });
        }
        Ok(applied)
    }

    /// Workflow #383 AO-3: the place of an applied invalidation once the gate captured its
    /// `fact.invalidated`.
    pub(crate) fn settle_invalidation(
        &mut self,
        data: &FactInvalidationEventData,
        order: FactOrder,
    ) {
        if let Some(tombstone) = self.invalidated.get_mut(&(
            (data.scope.clone(), data.key.clone()),
            data.source_snapshot_id.clone(),
        )) && tombstone.data == *data
        {
            tombstone.order = order;
        }
    }

    /// Workflow #384 D2-S3 (D2 model §3.6), through the store's one entry (Workflow #383 AO-3):
    /// drops `key`'s active record in `scope`, the one `fact.published` `base_event` carried,
    /// because the operation of the step whose `task.step_finished` is `trigger` changed the fact
    /// and no reading of it followed; one change of the store. `None` when the key has no active
    /// record or another record than that one; the record must stand before the trigger.
    pub(crate) fn apply_adjustment_invalidation(
        &mut self,
        scope: &FactScope,
        key: &str,
        base_event: EventId,
        trigger: &FactTrigger,
    ) -> RuntimeHostResult<Option<AppliedInvalidation>> {
        self.require_live(HostLocation::CommitFactInvalidation)?;
        let Some(stored) = self
            .active
            .get(&(scope.clone(), key.to_owned()))
            .filter(|stored| stored.event_id == base_event)
        else {
            return Ok(None);
        };
        let data = FactInvalidationEventData {
            scope: scope.clone(),
            key: key.to_owned(),
            source_snapshot_id: stored.record.source_snapshot_id.clone(),
            invalidated_at_unix_ms: trigger.at_unix_ms,
            invalidated_by_event_id: trigger.event_id,
            invalidated_by_event_type: trigger.event_type,
        };
        if trigger.order.is_none_or(|order| stored.order >= order)
            || !actingcommand_contract::is_adjustment_invalidation(&data)
        {
            return Err(fact_fatal(
                SharedCode::FactInvalidationTargetMismatch,
                HostLocation::CommitFactInvalidation,
            ));
        }
        self.apply_invalidation(data.clone(), UNSETTLED_ORDER, true)?;
        let revision = self.next_revision(HostLocation::CommitFactInvalidation)?;
        Ok(Some(AppliedInvalidation { data, revision }))
    }

    pub(crate) fn at_position(
        &self,
        ledger: &GlobalLedger,
        position: u64,
    ) -> RuntimeHostResult<Self> {
        if position == 0 || position > ledger.latest_sequence().map_err(fact_transaction_error)? {
            return Err(fact_fatal(
                HostCode::FactLedgerPositionInvalid,
                HostLocation::ProjectFactHistory,
            ));
        }
        let mut history = HistoricalFactProjection::default();
        for event in ledger
            .query(EventQuery {
                to_sequence: Some(position),
                ..EventQuery::default()
            })
            .map_err(|_| {
                fact_fatal(
                    HostCode::FactHistoryReadFailed,
                    HostLocation::ProjectFactHistory,
                )
            })?
        {
            history.replay(&event)?;
        }
        let active = history
            .active
            .into_iter()
            .map(|(identity, (record, sequence, event_id))| {
                let scope_instances =
                    history.scope_instances.remove(&identity).ok_or_else(|| {
                        fact_fatal(
                            HostCode::FactScopeProjectionMissing,
                            HostLocation::ProjectFactHistory,
                        )
                    })?;
                let publisher = history.publishers.remove(&identity).ok_or_else(|| {
                    fact_fatal(
                        HostCode::FactPublisherProjectionMissing,
                        HostLocation::ProjectFactHistory,
                    )
                })?;
                Ok((
                    identity,
                    StoredFact {
                        record,
                        order: FactOrder::Replayed(sequence),
                        sequence: Some(sequence),
                        event_id,
                        scope_instances,
                        publisher,
                    },
                ))
            })
            .collect::<RuntimeHostResult<_>>()?;
        Ok(Self {
            active,
            latest_observed: BTreeMap::new(),
            input_boundaries: history.input_boundaries,
            invalidated: BTreeMap::new(),
            pending: BTreeMap::new(),
            last_sequence: position,
            mode: StoreMode::Replaying,
            lineage: self.lineage,
            revision: self.revision,
            state: Arc::clone(&self.state),
        })
    }

    pub(crate) fn validate_pool_sources(
        &self,
        catalog: &actingcommand_policy::CompiledCatalog,
        mut current_scope_instances: impl FnMut(
            &FactScope,
        ) -> RuntimeHostResult<
            Vec<actingcommand_contract::InstanceId>,
        >,
    ) -> RuntimeHostResult<()> {
        for pool in &catalog.catalog().pools.pools {
            if pool.value_source.is_static() {
                continue;
            }
            let actingcommand_policy::ObservationRef::Fact { fact_key } = &pool.observation else {
                continue;
            };
            for stored in self.active.values().filter(|stored| {
                policy_scope(&stored.record.scope) == pool.scope && stored.record.key == *fact_key
            }) {
                // Workflow #390 POOL-1: a package reading is the pool's base until the next
                // reading or adjustment of its key resets it, so it needs no input invalidation.
                if stored.scope_instances.is_empty()
                    || stored.scope_instances != current_scope_instances(&stored.record.scope)?
                    || !(package_reading(stored)
                        || [EventType::InputCommitted, EventType::InputFailed]
                            .iter()
                            .all(|event| stored.record.invalidate_on.contains(event)))
                {
                    return Err(fact_request(
                        HostCode::PoolFactInvalidationBindingMissing,
                        HostLocation::ProjectFactPools,
                    ));
                }
            }
        }
        Ok(())
    }

    pub(crate) fn validate_input_boundaries(
        &self,
        record: &FactRecord,
        instances: &[actingcommand_contract::InstanceId],
    ) -> RuntimeHostResult<()> {
        self.input_boundaries.check(record, instances)
    }

    fn replay_event(
        &mut self,
        event: &PersistedEvent,
        ledger: &GlobalLedger,
    ) -> RuntimeHostResult<()> {
        let order = FactOrder::Replayed(event.sequence());
        match event.payload() {
            EventPayload::Fact(FactPayload::Published(payload)) => {
                self.input_boundaries
                    .check(payload.record(), payload.scope_instances())
                    .map_err(|_| {
                        fact_fatal(
                            HostCode::FactObservationPrecedesInput,
                            HostLocation::ReplayFactEvent,
                        )
                    })?;
                let mut complete = self.clone();
                for record in payload.records() {
                    if complete
                        .persisted_tombstone(
                            ledger,
                            &record.scope,
                            &record.key,
                            &record.source_snapshot_id,
                        )?
                        .is_some_and(|tombstone| tombstone.order <= order)
                    {
                        return Err(fact_fatal(
                            HostCode::FactSourceSnapshotRepublished,
                            HostLocation::ReplayFactEvent,
                        ));
                    }
                    complete.commit_publish(
                        record.clone(),
                        order,
                        Some(event.sequence()),
                        *event.event_id(),
                    )?;
                    let stored = complete
                        .active
                        .get_mut(&(record.scope.clone(), record.key.clone()))
                        .ok_or_else(|| {
                            fact_fatal(HostCode::FactCommitMissing, HostLocation::ReplayFactEvent)
                        })?;
                    stored.scope_instances = payload.scope_instances().to_vec();
                    stored.publisher = event.origin().actor();
                }
                complete.advance(event.sequence(), HostLocation::ReplayFactEvent)?;
                *self = complete;
                Ok(())
            }
            EventPayload::Fact(FactPayload::Invalidated(payload)) => {
                let data = payload.invalidation();
                let identity = (
                    (data.scope.clone(), data.key.clone()),
                    data.source_snapshot_id.clone(),
                );
                // Workflow #383 AO-3: an invalidation this start derived is applied already and
                // was written without a tombstone projection (the store keeps its tombstones;
                // the next start rebuilds the projection from the ledger); its record settles it.
                if self.pending.contains_key(&invalidation_identity(data))
                    && self
                        .invalidated
                        .get(&identity)
                        .is_some_and(|tombstone| tombstone.data == *data)
                {
                    self.apply_invalidation(data.clone(), order, true)?;
                    return self.advance(event.sequence(), HostLocation::ReplayFactEvent);
                }
                let entry = self
                    .persisted_tombstone(ledger, &data.scope, &data.key, &data.source_snapshot_id)?
                    .ok_or_else(|| {
                        fact_fatal(
                            SharedCode::FactTombstoneProjectionMissing,
                            HostLocation::ReplayFactEvent,
                        )
                    })?;
                if entry.data != *data || entry.order < order {
                    return Err(fact_fatal(
                        SharedCode::FactTombstoneProjectionSourceMismatch,
                        HostLocation::ReplayFactEvent,
                    ));
                }
                if !self.invalidated.contains_key(&identity)
                    && self.active.get(&identity.0).is_none_or(|active| {
                        active.record.source_snapshot_id != data.source_snapshot_id
                    })
                {
                    self.invalidated.insert(
                        identity,
                        InvalidationTombstone {
                            data: data.clone(),
                            order,
                        },
                    );
                    self.trim_recent_tombstones()?;
                    return self.advance(event.sequence(), HostLocation::ReplayFactEvent);
                }
                self.apply_invalidation(payload.invalidation().clone(), order, true)?;
                self.advance(event.sequence(), HostLocation::ReplayFactEvent)
            }
            _ => {
                self.input_boundaries.observe(event)?;
                for invalidation in self.plan_invalidations(event) {
                    self.derive_invalidation(invalidation, order)?;
                }
                self.advance(event.sequence(), HostLocation::ReplayFactEvent)
            }
        }
    }

    /// Workflow #383 AO-3: whether `record` may be published, from the store's memory only: an
    /// identity whose tombstone the store keeps is refused (`fact_source_snapshot_invalidated`);
    /// one whose tombstone rolled out is refused by the latest observation it saw
    /// (`fact_observation_not_newer`, `contracts/ledger-observer.md` §11.3). `Some` is the
    /// active publication the record repeats.
    pub(crate) fn preview_publish(
        &self,
        record: &FactRecord,
    ) -> RuntimeHostResult<Option<EventId>> {
        record.validate().map_err(|_| {
            fact_request(HostCode::FactRecordInvalid, ContractLocation::PublishFact)
        })?;
        let identity = (record.scope.clone(), record.key.clone());
        if self
            .invalidated
            .contains_key(&(identity.clone(), record.source_snapshot_id.clone()))
        {
            return Err(fact_request(
                HostCode::FactSourceSnapshotInvalidated,
                ContractLocation::PublishFact,
            ));
        }
        let Some(existing) = self.active.get(&identity) else {
            if self
                .latest_observed
                .get(&identity)
                .is_some_and(|latest| record.observed_at_unix_ms <= *latest)
            {
                return Err(fact_request(
                    HostCode::FactObservationNotNewer,
                    ContractLocation::PublishFact,
                ));
            }
            if self.active.len() >= MAX_ACTIVE_FACTS {
                return Err(fact_request(
                    HostCode::FactStoreCapacityExceeded,
                    ContractLocation::PublishFact,
                ));
            }
            return Ok(None);
        };
        if existing.record.source_snapshot_id != record.source_snapshot_id {
            if record.observed_at_unix_ms <= existing.record.observed_at_unix_ms {
                return Err(fact_request(
                    HostCode::FactObservationNotNewer,
                    ContractLocation::PublishFact,
                ));
            }
            return Ok(None);
        }
        if existing.record == *record {
            return Ok(Some(existing.event_id));
        }
        Err(fact_fatal(
            HostCode::FactSourceSnapshotIdentityConflict,
            ContractLocation::PublishFact,
        ))
    }

    pub(crate) fn commit_publish(
        &mut self,
        record: FactRecord,
        order: FactOrder,
        sequence: Option<u64>,
        event_id: EventId,
    ) -> RuntimeHostResult<()> {
        record
            .validate()
            .map_err(|_| fact_fatal(HostCode::FactRecordInvalid, HostLocation::CommitFact))?;
        if sequence == Some(0) {
            return Err(fact_fatal(
                HostCode::FactSequenceInvalid,
                HostLocation::CommitFact,
            ));
        }
        let identity = (record.scope.clone(), record.key.clone());
        if self
            .invalidated
            .contains_key(&(identity.clone(), record.source_snapshot_id.clone()))
        {
            return Err(fact_fatal(
                HostCode::FactSourceSnapshotRepublished,
                HostLocation::CommitFact,
            ));
        }
        if !self.active.contains_key(&identity) && self.active.len() >= MAX_ACTIVE_FACTS {
            return Err(fact_fatal(
                HostCode::FactStoreCapacityExceeded,
                HostLocation::CommitFact,
            ));
        }
        if let Some(existing) = self.active.get(&identity)
            && existing.record.source_snapshot_id == record.source_snapshot_id
        {
            if existing.record != record || existing.event_id != event_id {
                return Err(fact_fatal(
                    HostCode::FactSourceSnapshotIdentityConflict,
                    HostLocation::CommitFact,
                ));
            }
            return Ok(());
        }
        self.active.insert(
            identity.clone(),
            StoredFact {
                record,
                order,
                sequence,
                event_id,
                scope_instances: Vec::new(),
                // The caller binds the publishing event's actor right after the commit.
                publisher: EventActor::Runtime,
            },
        );
        let observed = self.active[&identity].record.observed_at_unix_ms;
        self.latest_observed
            .entry(identity)
            .and_modify(|latest| *latest = (*latest).max(observed))
            .or_insert(observed);
        Ok(())
    }

    pub(crate) fn preview_observation(
        &self,
        observation: &actingcommand_contract::FactObservation,
        now: u64,
    ) -> RuntimeHostResult<Option<EventId>> {
        observation.validate().map_err(|_| {
            fact_request(
                HostCode::FactObservationInvalid,
                ContractLocation::PublishFacts,
            )
        })?;
        let first = &observation.records[0];
        if first.observed_at_unix_ms > now {
            return Err(fact_request(
                HostCode::FactObservationInFuture,
                ContractLocation::PublishFacts,
            ));
        }
        let mut reused = BTreeSet::new();
        let mut new_count = 0;
        let mut new_identities = 0;
        for record in &observation.records {
            match self.preview_publish(record)? {
                Some(event) => {
                    reused.insert(event);
                }
                None => new_count += 1,
            }
            if !self
                .latest_observed
                .contains_key(&(record.scope.clone(), record.key.clone()))
            {
                new_identities += 1;
            }
        }
        let existing_members = self
            .active
            .values()
            .filter(|stored| {
                stored.record.scope == first.scope
                    && stored.record.source_snapshot_id == first.source_snapshot_id
            })
            .count();
        if new_count > 0
            && observation
                .records
                .iter()
                .filter_map(|record| self.active.get(&(record.scope.clone(), record.key.clone())))
                .any(|replaced| {
                    self.active.values().any(|member| {
                        member.event_id == replaced.event_id
                            && !observation.records.iter().any(|record| {
                                record.scope == member.record.scope
                                    && record.key == member.record.key
                            })
                    })
                })
        {
            return Err(fact_request(
                HostCode::FactObservationIncompleteRefresh,
                ContractLocation::PublishFacts,
            ));
        }
        if reused.len() > 1
            || (!reused.is_empty()
                && (new_count != 0 || existing_members != observation.records.len()))
            || (reused.is_empty() && existing_members != 0)
        {
            return Err(fact_request(
                HostCode::FactObservationVersionConflict,
                ContractLocation::PublishFacts,
            ));
        }
        if self.latest_observed.len() + new_identities > MAX_ACTIVE_FACTS {
            return Err(fact_request(
                HostCode::FactStoreCapacityExceeded,
                ContractLocation::PublishFacts,
            ));
        }
        Ok(reused.into_iter().next())
    }

    /// The `invalidate_on` invalidations a replayed event derives: the active records that stand
    /// before it and name its type, in its scope.
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
        let order = FactOrder::Replayed(event.sequence());
        self.active
            .values()
            .filter(|stored| {
                stored.order <= order
                    && stored.record.invalidate_on.contains(&event.event_type())
                    && invalidation_scope_matches(event, &stored.scope_instances)
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

    /// A replayed invalidation the start wrote: the store had applied it when it derived it; it
    /// is no longer to be written, and its record, once the store follows the ledger past it,
    /// settles it.
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
                HostCode::FactGeneratedInvalidationMissing,
                HostLocation::AcknowledgeFactInvalidation,
            )
        })?;
        let identity = invalidation_identity(data);
        if existing.data != *data || !self.pending.contains_key(&identity) {
            return Err(fact_fatal(
                HostCode::FactGeneratedInvalidationMismatch,
                HostLocation::AcknowledgeFactInvalidation,
            ));
        }
        if let Some(pending) = self.pending.get_mut(&identity) {
            pending.written = true;
        }
        Ok(())
    }

    fn derive_invalidation(
        &mut self,
        data: FactInvalidationEventData,
        order: FactOrder,
    ) -> RuntimeHostResult<()> {
        if !self
            .active
            .contains_key(&(data.scope.clone(), data.key.clone()))
        {
            return Err(fact_fatal(
                HostCode::FactInvalidationTargetMissing,
                HostLocation::DeriveFactInvalidation,
            ));
        }
        self.apply_invalidation(data.clone(), order, false)?;
        self.pending.insert(
            invalidation_identity(&data),
            PendingFactInvalidation {
                data,
                written: false,
            },
        );
        Ok(())
    }

    /// Applies one invalidation to memory. `settled` is false only for one the replay derived
    /// and the start has not written yet (it stays pending and is never compacted).
    fn apply_invalidation(
        &mut self,
        data: FactInvalidationEventData,
        order: FactOrder,
        settled: bool,
    ) -> RuntimeHostResult<()> {
        let identity = (data.scope.clone(), data.key.clone());
        let tombstone_key = (identity.clone(), data.source_snapshot_id.clone());
        if let Some(existing) = self.invalidated.get(&tombstone_key) {
            if existing.data != data {
                return Err(fact_fatal(
                    SharedCode::FactInvalidationIdentityConflict,
                    HostLocation::CommitFactInvalidation,
                ));
            }
            if settled {
                self.pending.remove(&invalidation_identity(&data));
                let tombstone = self.invalidated.get_mut(&tombstone_key).ok_or_else(|| {
                    fact_fatal(
                        HostCode::FactInvalidationTargetMissing,
                        HostLocation::CommitFactInvalidation,
                    )
                })?;
                tombstone.order = order;
                self.trim_recent_tombstones()?;
            }
            return Ok(());
        }
        let active = self.active.get(&identity).ok_or_else(|| {
            fact_fatal(
                HostCode::FactInvalidationTargetMissing,
                HostLocation::CommitFactInvalidation,
            )
        })?;
        if active.record.source_snapshot_id != data.source_snapshot_id
            || !(active
                .record
                .invalidate_on
                .contains(&data.invalidated_by_event_type)
                || actingcommand_contract::is_adjustment_invalidation(&data))
        {
            return Err(fact_fatal(
                SharedCode::FactInvalidationTargetMismatch,
                HostLocation::CommitFactInvalidation,
            ));
        }
        self.active.remove(&identity);
        self.invalidated.insert(
            tombstone_key,
            InvalidationTombstone {
                data: data.clone(),
                order,
            },
        );
        self.latest_observed
            .entry(identity.clone())
            .and_modify(|at| *at = (*at).max(data.invalidated_at_unix_ms))
            .or_insert(data.invalidated_at_unix_ms);
        if settled {
            self.trim_recent_tombstones()?;
        }
        Ok(())
    }

    /// The tombstone projection of the state document (written by the starts before AO-3 and
    /// rebuilt from the ledger by every start): read by the start's replay only.
    fn persisted_tombstone(
        &self,
        ledger: &GlobalLedger,
        scope: &FactScope,
        key: &str,
        source_snapshot_id: &str,
    ) -> RuntimeHostResult<Option<InvalidationTombstone>> {
        let entry_key =
            fact_tombstone_key(scope, key, source_snapshot_id).map_err(fact_state_error)?;
        let Some(entry) = self
            .state
            .read_projection_entry(FACT_TOMBSTONE_NAMESPACE, &entry_key)
            .map_err(fact_state_error)?
        else {
            return Ok(None);
        };
        let original = original_at(ledger, entry.ledger_sequence())?;
        self.state
            .verify_fact_projection_entry(&entry, &original)
            .map_err(fact_state_error)?;
        let data =
            serde_json::from_slice::<FactInvalidationEventData>(entry.payload()).map_err(|_| {
                fact_fatal(
                    HostCode::FactTombstoneProjectionInvalid,
                    HostLocation::ReadFactTombstone,
                )
            })?;
        if &data.scope != scope || data.key != key || data.source_snapshot_id != source_snapshot_id
        {
            return Err(fact_fatal(
                HostCode::FactTombstoneProjectionIdentityMismatch,
                HostLocation::ReadFactTombstone,
            ));
        }
        Ok(Some(InvalidationTombstone {
            data,
            order: FactOrder::Replayed(entry.ledger_sequence()),
        }))
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
                .min_by_key(|(identity, tombstone)| (tombstone.order, *identity))
                .map(|(identity, _)| identity.clone())
                .ok_or_else(|| {
                    fact_fatal(
                        HostCode::FactTombstoneCompactionBlocked,
                        HostLocation::CompactFactTombstones,
                    )
                })?;
            self.invalidated.remove(&candidate);
        }
        Ok(())
    }

    fn advance(&mut self, sequence: u64, operation: impl Into<Location>) -> RuntimeHostResult<()> {
        let operation: Location = operation.into();
        if sequence == 0 || sequence < self.last_sequence {
            return Err(fact_fatal(HostCode::FactSequenceInvalid, operation));
        }
        self.last_sequence = sequence;
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
            self.derive_invalidation(data, FactOrder::Replayed(self.last_sequence + 1))
                .expect("test trigger invalidation");
        }
    }

    /// The active record stored under exactly this scope and key, if any.
    pub(crate) fn active_record(&self, scope: &FactScope, key: &str) -> Option<&FactRecord> {
        self.active
            .get(&(scope.clone(), key.to_owned()))
            .map(|stored| &stored.record)
    }

    /// The active record under exactly this scope and key with the place, the ledger sequence
    /// when known and the id of the `fact.published` event that carried it (Workflow #308
    /// RT-S1a; Workflow #383 AO-3).
    pub(crate) fn active_revision(&self, scope: &FactScope, key: &str) -> Option<ActiveFact<'_>> {
        self.active
            .get(&(scope.clone(), key.to_owned()))
            .map(|stored| ActiveFact {
                record: &stored.record,
                order: stored.order,
                sequence: stored.sequence,
                event_id: stored.event_id,
            })
    }

    /// The active record for `key` that applies to `context`, the most specific scope first
    /// (instance over server over game); `None` when no active record applies.
    pub(crate) fn resolve_active(
        &self,
        context: &InstanceFactContext,
        key: &str,
    ) -> Option<&FactRecord> {
        self.active
            .values()
            .map(|stored| &stored.record)
            .filter(|record| record.key == key && record.scope.matches(context))
            .max_by_key(|record| scope_specificity(&record.scope))
    }

    /// The active records that apply to one instance (Workflow #389 MCP-4): its own, and those
    /// of its server and game when `shared` names them (server, game); ordered as `snapshot`
    /// orders them, by key, then by scope.
    pub(crate) fn instance_records(
        &self,
        instance_alias: &str,
        shared: Option<(&str, &str)>,
    ) -> Vec<FactRecord> {
        let mut records = self
            .active
            .values()
            .map(|stored| &stored.record)
            .filter(|record| match &record.scope {
                FactScope::Instance { instance_id } => instance_id == instance_alias,
                FactScope::Server { server_id } => {
                    shared.is_some_and(|(server, _)| server == server_id)
                }
                FactScope::Game { game_id } => shared.is_some_and(|(_, game)| game == game_id),
            })
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by(|left, right| {
            left.key
                .cmp(&right.key)
                .then_with(|| left.scope.cmp(&right.scope))
        });
        records
    }

    /// The records that apply to `context`, identified by the two stores' (lineage, revision)
    /// and the records (Workflow #383 AO-3, `contracts/ledger-observer.md` §13): the snapshot id
    /// no longer depends on the ledger position, which the snapshot still names.
    pub(crate) fn snapshot(
        &self,
        context: InstanceFactContext,
        ledger_position: u64,
        workbench: RuntimeStateStoreRevision,
    ) -> RuntimeHostResult<InstanceFactSnapshot> {
        context.validate().map_err(|_| {
            fact_request(HostCode::FactContextInvalid, HostLocation::ReadFactSnapshot)
        })?;
        if ledger_position == 0 {
            return Err(fact_fatal(
                HostCode::FactLedgerPositionInvalid,
                HostLocation::ReadFactSnapshot,
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
        let snapshot_id = snapshot_id(&context, self.revision(), workbench, &records)?;
        let snapshot = InstanceFactSnapshot {
            snapshot_id,
            ledger_position,
            context,
            records,
        };
        snapshot.validate().map_err(|_| {
            fact_fatal(
                HostCode::FactSnapshotInvalid,
                HostLocation::ReadFactSnapshot,
            )
        })?;
        Ok(snapshot)
    }

    /// The evaluation inputs with the store's records overlaid. Their snapshot id is computed
    /// from the inputs, the records and the two stores' (lineage, revision), never from a ledger
    /// position (Workflow #383 AO-3, §13).
    pub(crate) fn overlay_policy_facts(
        &self,
        facts: &EvaluationFacts,
        resources: &EvaluationResources,
        ledger_position: u64,
        workbench: RuntimeStateStoreRevision,
    ) -> RuntimeHostResult<EvaluationFacts> {
        self.overlay_policy_facts_with_disposition(
            facts,
            resources,
            (ledger_position, workbench),
            PolicyFactConflictDisposition::FatalInvariant,
            HostLocation::ProjectPolicyFacts,
        )
    }

    pub(crate) fn overlay_external_policy_facts(
        &self,
        facts: &EvaluationFacts,
        resources: &EvaluationResources,
        ledger_position: u64,
        workbench: RuntimeStateStoreRevision,
    ) -> RuntimeHostResult<EvaluationFacts> {
        self.overlay_policy_facts_with_disposition(
            facts,
            resources,
            (ledger_position, workbench),
            PolicyFactConflictDisposition::RejectCaller,
            ContractLocation::ProjectPolicyForward,
        )
    }

    fn overlay_policy_facts_with_disposition(
        &self,
        facts: &EvaluationFacts,
        resources: &EvaluationResources,
        (ledger_position, workbench): (u64, RuntimeStateStoreRevision),
        conflict_disposition: PolicyFactConflictDisposition,
        operation: impl Into<Location>,
    ) -> RuntimeHostResult<EvaluationFacts> {
        let operation: Location = operation.into();
        // Workflow #308 RT-S1a: a resource target policy is written only by its formal entry
        // into the store; neither the configuration nor a forward caller may supply one.
        if facts
            .facts
            .iter()
            .any(|fact| fact.fact_key == RESOURCE_TARGETS_FACT_KEY)
        {
            return Err(RuntimeHostError::request(
                HostCode::ResourceTargetsKeyReserved,
                operation,
                RuntimeErrorCode::InvalidRequest,
            ));
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
            // The instance projection consumes the policy instance keys and the priority
            // offset projection consumes the offset keys; neither is an ordinary fact.
            if is_policy_instance_fact_key(&stored.record.key)
                || priority_offset_task_id(&stored.record.key).is_some()
            {
                continue;
            }
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
                        fact_fatal(HostCode::PolicyFactAuthorityConflict, operation)
                    }
                    PolicyFactConflictDisposition::RejectCaller => RuntimeHostError::request(
                        HostCode::PolicyFactAuthorityConflict,
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
        // Workflow #383 AO-3: a record is identified by its publication, not by its ledger
        // position.
        let authority_revisions = selected
            .iter()
            .map(|((scope, key), stored)| {
                (
                    scope,
                    key,
                    &stored.record.source_snapshot_id,
                    stored.event_id,
                )
            })
            .collect::<Vec<_>>();
        // The authoritative projection identifies the priority offsets it consumed by their
        // (scope, task, offset, observation time); an external projection consumes none.
        let offset_identity: Vec<PriorityOffsetIdentity> = match conflict_disposition {
            PolicyFactConflictDisposition::FatalInvariant => self
                .active_priority_offsets(&contexts, operation)?
                .into_iter()
                .map(|offset| {
                    (
                        offset.scope,
                        offset.task_id,
                        offset.offset_milli,
                        offset.observed_at_unix_ms,
                    )
                })
                .collect(),
            PolicyFactConflictDisposition::RejectCaller => Vec::new(),
        };
        projected.ledger_position = ledger_position;
        projected.fact_snapshot_id = combined_policy_snapshot_id(
            &projected,
            resources,
            &authority_revisions,
            &offset_identity,
            (self.revision(), workbench),
        )?;
        Ok(projected)
    }

    /// Every active priority offset record whose scope applies to at least one of
    /// `contexts`, in store order (scope, then key). A record whose task identifier or value
    /// breaks the publish rules is refused as `priority_offset_invalid`, never skipped.
    pub(crate) fn active_priority_offsets(
        &self,
        contexts: &[InstanceFactContext],
        operation: impl Into<Location>,
    ) -> RuntimeHostResult<Vec<ActivePriorityOffset>> {
        let operation: Location = operation.into();
        let mut offsets = Vec::new();
        for stored in self.active.values() {
            let Some(task_id) = priority_offset_task_id(&stored.record.key) else {
                continue;
            };
            if !contexts
                .iter()
                .any(|context| stored.record.scope.matches(context))
            {
                continue;
            }
            let offset_milli = priority_offset_milli(&stored.record)
                .filter(|_| valid_priority_offset_task_id(task_id))
                .ok_or_else(|| {
                    RuntimeHostError::request(
                        SharedCode::PriorityOffsetInvalid,
                        operation,
                        RuntimeErrorCode::InvalidRequest,
                    )
                })?;
            offsets.push(ActivePriorityOffset {
                scope: stored.record.scope.clone(),
                task_id: task_id.to_owned(),
                offset_milli,
                origin: priority_offset_origin(stored.publisher),
                observed_at_unix_ms: stored.record.observed_at_unix_ms,
                expires_at_unix_ms: stored.record.expires_at_unix_ms,
            });
        }
        Ok(offsets)
    }

    #[cfg(test)]
    fn active_count(&self) -> usize {
        self.active.len()
    }
}

/// Builds the evaluation's manual priority offsets from active offset records (Workflow #308
/// slice 4a-2). An instance-scoped record is the entry of that instance. A task's single
/// server- or game-scoped record that applies to every evaluated instance is the task-level
/// entry (`instance_id: None`), which the evaluator applies to every instance without an
/// instance-level entry of its own. When a task's server/game records apply to only some of
/// the evaluated instances, or more than one of them applies, no single task-level entry can
/// express them: every instance then gets its own entry from its most specific record
/// (instance over server over game), the store's own resolution order.
pub(crate) fn resolve_priority_offsets(
    offsets: &[ActivePriorityOffset],
    instances: &[InstanceSnapshot],
) -> Vec<actingcommand_policy::PriorityOffset> {
    let contexts = instances.iter().map(instance_context).collect::<Vec<_>>();
    let mut by_task = BTreeMap::<&str, Vec<&ActivePriorityOffset>>::new();
    for offset in offsets {
        by_task
            .entry(offset.task_id.as_str())
            .or_default()
            .push(offset);
    }
    let entry = |offset: &ActivePriorityOffset, instance_id: Option<String>| {
        actingcommand_policy::PriorityOffset {
            task_id: offset.task_id.clone(),
            instance_id,
            offset_milli: offset.offset_milli,
            origin: offset.origin,
            observed_at_unix_ms: offset.observed_at_unix_ms,
        }
    };
    let mut resolved = Vec::new();
    for records in by_task.values() {
        let shared = records
            .iter()
            .filter(|offset| !matches!(offset.scope, FactScope::Instance { .. }))
            .collect::<Vec<_>>();
        match shared.as_slice() {
            [] => {}
            [only] if contexts.iter().all(|context| only.scope.matches(context)) => {
                resolved.push(entry(only, None));
            }
            _ => {
                for context in &contexts {
                    if let Some(most_specific) = records
                        .iter()
                        .filter(|offset| offset.scope.matches(context))
                        .max_by_key(|offset| scope_specificity(&offset.scope))
                    {
                        resolved.push(entry(most_specific, Some(context.instance_id.clone())));
                    }
                }
                continue;
            }
        }
        for offset in records {
            if let FactScope::Instance { instance_id } = &offset.scope {
                resolved.push(entry(offset, Some(instance_id.clone())));
            }
        }
    }
    resolved
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

pub(crate) fn policy_scope(scope: &FactScope) -> ScopeSelector {
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

/// Workflow #383 AO-3 (`contracts/ledger-observer.md` §13): `hash(context, instance store
/// (lineage, revision), workbench (lineage, revision), records)`.
fn snapshot_id(
    context: &InstanceFactContext,
    instance_facts: RuntimeStateStoreRevision,
    runtime_facts: RuntimeStateStoreRevision,
    records: &[FactRecord],
) -> RuntimeHostResult<String> {
    let bytes = serde_json::to_vec(&(
        context,
        (instance_facts.lineage, instance_facts.revision),
        (runtime_facts.lineage, runtime_facts.revision),
        records,
    ))
    .map_err(|_| {
        fact_fatal(
            HostCode::FactSnapshotEncodeFailed,
            HostLocation::HashFactSnapshot,
        )
    })?;
    Ok(format!("snapshot:fact:{:x}", Sha256::digest(bytes)))
}

/// The consumed priority offsets' identity: (scope, task, offset in milli, observed at).
type PriorityOffsetIdentity = (FactScope, String, i32, u64);

/// The policy inputs' snapshot id: the inputs, the records they took from the store, the
/// consumed priority offsets and the two stores' (lineage, revision) (Workflow #383 AO-3, §13).
fn combined_policy_snapshot_id<T: serde::Serialize>(
    facts: &EvaluationFacts,
    resources: &EvaluationResources,
    authority_revisions: &[T],
    offset_identity: &[PriorityOffsetIdentity],
    (instance_facts, runtime_facts): (RuntimeStateStoreRevision, RuntimeStateStoreRevision),
) -> RuntimeHostResult<String> {
    let mut resources = resources.clone();
    resources
        .pools
        .sort_by(|left, right| left.pool_id.cmp(&right.pool_id));
    resources
        .hosts
        .sort_by(|left, right| left.host_id.cmp(&right.host_id));
    let revisions = (
        (instance_facts.lineage, instance_facts.revision),
        (runtime_facts.lineage, runtime_facts.revision),
    );
    // Without offsets the hashed tuple leaves them out, as before offsets existed.
    let bytes = if offset_identity.is_empty() {
        serde_json::to_vec(&(
            &facts.facts,
            &facts.outcomes,
            &facts.tasks,
            &facts.instances,
            &resources,
            authority_revisions,
            revisions,
        ))
    } else {
        serde_json::to_vec(&(
            &facts.facts,
            &facts.outcomes,
            &facts.tasks,
            &facts.instances,
            &resources,
            authority_revisions,
            offset_identity,
            revisions,
        ))
    }
    .map_err(|_| {
        fact_fatal(
            HostCode::FactSnapshotEncodeFailed,
            HostLocation::HashPolicyFactSnapshot,
        )
    })?;
    Ok(format!("snapshot:policy-fact:{:x}", Sha256::digest(bytes)))
}

fn fact_request(code: impl Into<Code>, operation: impl Into<Location>) -> RuntimeHostError {
    RuntimeHostError::request(code, operation, RuntimeErrorCode::InvalidRequest)
}

fn fact_fatal(code: impl Into<Code>, operation: impl Into<Location>) -> RuntimeHostError {
    RuntimeHostError::fatal(code, operation, RuntimeErrorCode::RuntimeHalted)
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
        let issuer = actingcommand_contract::IdentifierIssuer::new().expect("issuer");
        InstanceFactStore {
            active: BTreeMap::new(),
            latest_observed: BTreeMap::new(),
            input_boundaries: InputInvalidationBoundaries::default(),
            invalidated: BTreeMap::new(),
            pending: BTreeMap::new(),
            last_sequence: 0,
            mode: StoreMode::Replaying,
            lineage: *issuer.mint_owner_epoch().expect("lineage").transport(),
            revision: 0,
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
                heavy_threshold_milli: actingcommand_policy::DEFAULT_HEAVY_THRESHOLD_MILLI,
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
                FactOrder::Replayed(1),
                Some(1),
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
                FactOrder::Replayed(2),
                Some(2),
                *issuer.mint_event_id().expect("event").transport(),
            )
            .expect("instance fact");
        let workbench = store.revision();
        let a = store
            .snapshot(
                InstanceFactContext {
                    instance_id: "instance-a".to_owned(),
                    server_id: "server-a".to_owned(),
                    game_id: "game-a".to_owned(),
                },
                2,
                workbench,
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
                workbench,
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
                FactOrder::Replayed(1),
                Some(1),
                published,
            )
            .expect("publish");
        store.apply_trigger(EventType::RuntimeTakeover, trigger, 2_000);
        assert_eq!(store.active_count(), 0);

        // LIVE-FACT-POOL-v1 specification: source time survives invalidation.
        let old = record(
            FactScope::Instance {
                instance_id: "instance-a".to_owned(),
            },
            "snapshot:old",
            vec![EventType::RuntimeTakeover],
        );
        assert_eq!(
            store
                .preview_observation(
                    &actingcommand_contract::FactObservation {
                        records: vec![old.clone()]
                    },
                    3_000,
                )
                .unwrap_err()
                .code(),
            "fact_observation_not_newer"
        );
        let mut fresh = old;
        fresh.observed_at_unix_ms = 2_001;
        assert!(
            store
                .preview_observation(
                    &actingcommand_contract::FactObservation {
                        records: vec![fresh.clone()]
                    },
                    3_000,
                )
                .is_ok()
        );
        assert_eq!(
            store
                .preview_observation(
                    &actingcommand_contract::FactObservation {
                        records: vec![fresh]
                    },
                    2_000,
                )
                .unwrap_err()
                .code(),
            "fact_observation_in_future"
        );
    }

    #[test]
    fn policy_snapshot_identity_covers_every_evaluator_fact_dimension() {
        let (_root, store) = empty_store();
        let workbench = store.revision();
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
                expires_at_unix_ms: None,
                activity_window_id: None,
            }],
            tasks: vec![TaskRuntimeSnapshot {
                task_id: "task-a".to_owned(),
                instance_id: "instance-a".to_owned(),
                last_dispatched_unix_ms: None,
                eligible_since_unix_ms: Some(1_000),
                terminal_state: None,
                completed_window: None,
                last_duration_ms: None,
                failure_streak: None,
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
            priority_offsets: Vec::new(),
        };
        let resources = resources();
        let canonical = store
            .overlay_policy_facts(&base, &resources, 7, workbench)
            .expect("canonical facts");

        let mut caller_identity_changed = base.clone();
        caller_identity_changed.fact_snapshot_id = "caller-snapshot-b".to_owned();
        assert_eq!(
            store
                .overlay_policy_facts(&caller_identity_changed, &resources, 99, workbench)
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
                    .overlay_policy_facts(&changed, &resources, 7, workbench)
                    .expect("changed facts")
                    .fact_snapshot_id,
                canonical.fact_snapshot_id
            );
        }

        let mut changed_resources = resources.clone();
        changed_resources.pools[0].value += 1;
        assert_ne!(
            store
                .overlay_policy_facts(&base, &changed_resources, 7, workbench)
                .expect("changed resources")
                .fact_snapshot_id,
            canonical.fact_snapshot_id
        );
    }

    /// Workflow #383 AO-3 (`contracts/ledger-observer.md` §3.6, §11.3): the store keeps at most
    /// 256 tombstones in memory; an identity whose tombstone rolled out is still refused, by the
    /// latest observation of its key (`fact_observation_not_newer`), live and after the start's
    /// replay rebuilt the store from the ledger.
    #[test]
    fn fact_tombstones_compact_in_memory_without_losing_durable_rejection() {
        use actingcommand_contract::{
            AuditInput, EventAction, EventActor, EventDraft, EventLinksDraft, EventOrigin,
            EventSeverity, EventSource, FactPayloadDraft, OriginModule, RuntimePayloadDraft,
        };
        use actingcommand_ledger::{GlobalLedgerConfig, Sha256SecretFingerprinter};
        let root = TempDir::new().expect("tempdir");
        let database = Arc::new(
            RuntimeStateStore::open_database(root.path(), b"0123456789abcdef").expect("database"),
        );
        let state =
            Arc::new(RuntimeStateStore::from_database(Arc::clone(&database)).expect("state store"));
        let ledger = GlobalLedger::open_sqlite_candidate(
            GlobalLedgerConfig::new(root.path(), "fact-compaction"),
            database,
        )
        .expect("ledger");
        let mut store = store_with_state(Arc::clone(&state));
        store.go_live().expect("live store");
        let lineage = store.revision().lineage;
        let issuer = actingcommand_contract::IdentifierIssuer::new().expect("issuer");
        let fingerprinter =
            Sha256SecretFingerprinter::new(b"fact-compaction-specification").expect("salt");
        let scope = FactScope::Instance {
            instance_id: "instance-a".to_owned(),
        };
        let mut first = None;
        let mut capture = 0;

        for index in 0..(MAX_RECENT_FACT_TOMBSTONES + 32) {
            let snapshot = format!("snapshot:compaction-{index}");
            let fact = record(scope.clone(), &snapshot, vec![EventType::RuntimeTakeover]);
            first.get_or_insert_with(|| fact.clone());
            let published_id = issuer.mint_event_id().expect("publish event");
            let published_event = *published_id.transport();
            store
                .apply_publication(
                    std::slice::from_ref(&fact),
                    &[],
                    EventActor::Runtime,
                    published_event,
                )
                .expect("live publication");
            capture += 1;
            store.settle_publication(published_event, FactOrder::Captured(capture), None);
            let published = EventDraft::new(
                published_id,
                1_000,
                EventSeverity::Info,
                EventOrigin::new(
                    EventSource::Runtime,
                    OriginModule::FactStore,
                    EventActor::Runtime,
                ),
                EventLinksDraft::default(),
                FactPayloadDraft::published(fact, AuditInput::new()).into(),
            )
            .sanitize(&fingerprinter)
            .expect("publish draft");
            ledger.append(published).expect("publish original fact");
            let trigger_id = issuer.mint_event_id().expect("trigger event");
            let trigger_event = *trigger_id.transport();
            let trigger = EventDraft::new(
                trigger_id,
                2_000 + index as u64,
                EventSeverity::Info,
                EventOrigin::new(
                    EventSource::Runtime,
                    OriginModule::Runtime,
                    EventActor::Runtime,
                ),
                EventLinksDraft::default(),
                RuntimePayloadDraft::takeover(EventAction::RuntimeTakeover, AuditInput::new())
                    .into(),
            )
            .sanitize(&fingerprinter)
            .expect("trigger draft");
            let applied = store
                .observe_trigger(&FactTrigger::at_capture(&trigger))
                .expect("observe the trigger");
            assert_eq!(applied.len(), 1, "the trigger invalidates its record");
            assert_eq!(trigger_event, applied[0].data.invalidated_by_event_id);
            ledger.append(trigger).expect("original trigger");
            capture += 2;
            store.settle_invalidation(&applied[0].data, FactOrder::Captured(capture));
            let draft = EventDraft::new(
                issuer.mint_event_id().expect("invalidation event"),
                2_000 + index as u64,
                EventSeverity::Info,
                EventOrigin::new(
                    EventSource::Runtime,
                    OriginModule::FactStore,
                    EventActor::Runtime,
                ),
                EventLinksDraft::default(),
                FactPayloadDraft::invalidated(applied[0].data.clone(), AuditInput::new()).into(),
            )
            .sanitize(&fingerprinter)
            .expect("invalidation draft");
            ledger.append(draft).expect("invalidation");
        }

        assert_eq!(store.invalidated.len(), MAX_RECENT_FACT_TOMBSTONES);
        assert_eq!(store.active_count(), 0);
        let first = first.expect("first fact");
        assert_eq!(
            store
                .preview_publish(&first)
                .expect_err("a rolled-out tombstone's identity is still refused")
                .code(),
            "fact_observation_not_newer"
        );

        drop(store);
        let recovered =
            InstanceFactStore::recover(&ledger, state, lineage).expect("replay original facts");
        assert_eq!(
            recovered
                .preview_publish(&first)
                .expect_err("the replayed store still refuses the identity")
                .code(),
            "fact_observation_not_newer"
        );
    }
}
