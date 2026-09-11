// SPDX-License-Identifier: AGPL-3.0-only

use super::{GlobalLedgerError, GlobalLedgerResult, projection::EventIndexes};
use crate::fact::{FactValidationError, LedgerEventRead, StoredEventRecord};
use crate::{ArtifactAvailability, PersistedEvent};
use actingcommand_artifact_store::ArtifactDeleteGuard;
use actingcommand_contract::{
    ArtifactEvictionDisposition, ArtifactEvictionIntentRecord, ArtifactEvictionProof, ArtifactId,
    ArtifactKind, ArtifactPinReason, ArtifactRetentionFact, ArtifactRetentionIdentity,
    CapturePayload, CorrelationId, EffectDisposition, EventId, EventPayload, EventSeverity,
    EventSource, EventType, FRAME_RETENTION_BACKTRACE, FactContent, FactPayload, FrameId,
    InstanceId, LeaseId, LeasePayload, OwnerEpoch, PolicyExecutionOutcome, PolicyPayload,
    ProjectedArtifactReference, RETENTION_ROUND_OBJECTS, RequestId, RunId, RuntimeLifecyclePhase,
    RuntimePayload, RuntimeStateFact, TaskOutcome, TaskPayload, TaskSemanticFact, TerminalEvent,
    VerifiedArtifactReference,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Derived only from the authenticated prefix, inside the original Ledger owner.
#[derive(Default)]
pub(super) struct RetentionIndex {
    objects: BTreeMap<ArtifactId, RetainedObject>,
    frames: BTreeMap<FrameId, BTreeSet<ArtifactId>>,
    recent: BTreeMap<RetentionScope, VecDeque<FrameId>>,
    seen_frames: BTreeSet<FrameId>,
    linked_runs: BTreeMap<RetentionScope, BTreeSet<RunId>>,
    sealed_scopes: BTreeSet<RetentionScope>,
    scheduled_runs: BTreeSet<RunId>,
    owner_epochs: BTreeMap<u64, OwnerEpoch>,
    closures: BTreeMap<ClosureScope, ClosureFacts>,
    released_leases: BTreeMap<(OwnerEpoch, InstanceId, LeaseId), TerminalEvent>,
    pending_objects: BTreeSet<ArtifactId>,
    unlinked_warning: bool,
    through_sequence: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum RetentionScope {
    Run(RunId),
    Request(RequestId),
    Correlation(CorrelationId),
}

struct RetainedObject {
    reference: ProjectedArtifactReference,
    verified: Option<TerminalEvent>,
    summary: Option<TerminalEvent>,
    identity: Option<ArtifactRetentionIdentity>,
    pins: BTreeMap<EventId, (TerminalEvent, ArtifactPinReason)>,
    permanently_protected: bool,
    proof: Option<ArtifactEvictionProof>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct ClosureScope {
    owner: OwnerEpoch,
    instance: InstanceId,
    run: Option<RunId>,
    lease: Option<LeaseId>,
    request: Option<RequestId>,
    correlation: Option<CorrelationId>,
}

#[derive(Default)]
struct ClosureFacts {
    success: Option<TerminalEvent>,
    close: Option<TerminalEvent>,
    settlement: Option<TerminalEvent>,
}

impl ClosureScope {
    fn from_identity(identity: &ArtifactRetentionIdentity) -> Self {
        Self {
            owner: identity.owner_epoch,
            instance: identity.instance_id,
            run: identity.run_id,
            lease: identity.lease_id,
            request: identity.run_id.is_none().then_some(identity.request_id),
            correlation: identity.run_id.is_none().then_some(identity.correlation_id),
        }
    }

    fn from_event<E: LedgerEventRead>(event: &E, owner: OwnerEpoch) -> Option<Self> {
        let links = event.links();
        let run = links.run_id().copied();
        Some(Self {
            owner,
            instance: *links.instance_id()?,
            run,
            lease: links.lease_id().copied(),
            request: if run.is_none() {
                Some(*links.request_id()?)
            } else {
                None
            },
            correlation: if run.is_none() {
                Some(*links.correlation_id()?)
            } else {
                None
            },
        })
    }
}

/// Every returned object counts against the round, including protected objects.
pub struct ArtifactRetentionCandidates {
    pub references: Vec<ProjectedArtifactReference>,
    pub next_after: Option<ArtifactId>,
    pub through_sequence: u64,
    pub recovery_pending: bool,
}

/// The original writer returns this only after committing or validating the sealed intent.
pub struct ArtifactEvictionPermit {
    guard: ArtifactDeleteGuard,
    intent: ArtifactEvictionIntentRecord,
    source: TerminalEvent,
    recovery: bool,
}

pub enum ArtifactEvictionAdmission {
    Deferred,
    Committed(Box<ArtifactEvictionPermit>),
}

impl RetentionIndex {
    fn event_proofs<E: LedgerEventRead>(&self, event: &E) -> Vec<ArtifactEvictionProof> {
        let mut seen = BTreeSet::new();
        material_references(event)
            .into_iter()
            .filter(|reference| seen.insert(reference.artifact_id))
            .filter_map(|reference| self.objects.get(&reference.artifact_id)?.proof.clone())
            .map(|mut proof| {
                proof.through_sequence = self.through_sequence;
                proof
            })
            .collect()
    }

    pub(super) fn annotate_event(&self, event: &mut PersistedEvent) {
        event.apply_artifact_evictions(self.event_proofs(event));
    }

    pub(super) fn observations<E: LedgerEventRead>(
        &self,
        event: &E,
        snapshot: u64,
    ) -> Vec<actingcommand_contract::ArtifactEvictionObservation> {
        self.event_proofs(event)
            .iter()
            .filter_map(|proof| proof.observation(snapshot))
            .collect()
    }

    pub(super) fn has_pending(&self) -> bool {
        !self.pending_objects.is_empty()
    }

    pub(super) fn candidates(
        &self,
        after: Option<ArtifactId>,
        recovery_only: bool,
    ) -> ArtifactRetentionCandidates {
        use std::ops::Bound::{Excluded, Unbounded};
        let bounds = (after.map_or(Unbounded, Excluded), Unbounded);
        let references = if recovery_only {
            self.pending_objects
                .range(bounds)
                .take(RETENTION_ROUND_OBJECTS)
                .map(|id| {
                    self.objects
                        .get(id)
                        .expect("pending object identity validated")
                        .reference
                        .clone()
                })
                .collect::<Vec<_>>()
        } else {
            self.objects
                .range(bounds)
                .take(RETENTION_ROUND_OBJECTS)
                .map(|(_, object)| object.reference.clone())
                .collect::<Vec<_>>()
        };
        let next_after = (references.len() == RETENTION_ROUND_OBJECTS)
            .then(|| references.last().expect("nonempty full page").artifact_id);
        ArtifactRetentionCandidates {
            references,
            next_after,
            through_sequence: self.through_sequence,
            recovery_pending: recovery_only,
        }
    }

    /// Source selection uses indexes updated by the same committed writer.
    pub(super) fn intent<E: LedgerEventRead>(
        &self,
        reference: &ProjectedArtifactReference,
        events: &[E],
        indexes: &EventIndexes,
    ) -> GlobalLedgerResult<Option<ArtifactEvictionIntentRecord>> {
        let Some(object) = self.objects.get(&reference.artifact_id) else {
            return Ok(None);
        };
        if object.reference != *reference {
            return Err(invalid("artifact_identity_conflict"));
        }
        if object.proof.is_some()
            || object.permanently_protected
            || self.unlinked_warning
            || !object.pins.is_empty()
        {
            return Ok(None);
        }
        let (Some(identity), Some(verified)) = (&object.identity, &object.verified) else {
            return Ok(None);
        };
        let Some(closure) = self.closures.get(&ClosureScope::from_identity(identity)) else {
            return Ok(None);
        };
        let (Some(success), Some(close)) = (&closure.success, self.close_for(identity)) else {
            return Ok(None);
        };
        if verified.sequence > success.sequence
            || success.sequence >= close.sequence
            || identity.run_id.is_some() && object.summary.is_none()
            || identity
                .run_id
                .is_some_and(|run| self.scheduled_runs.contains(&run))
                && closure.settlement.is_none()
        {
            return Ok(None);
        }
        let intent = ArtifactEvictionIntentRecord {
            identity: identity.clone(),
            verified: verified.clone(),
            success: success.clone(),
            close: close.clone(),
            capture_summary: object.summary.clone(),
            settlement: closure.settlement.clone(),
            through_sequence: self.through_sequence,
        };
        self.validate_intent(object, &intent, events, indexes)?;
        Ok(Some(intent))
    }

    fn close_for(&self, identity: &ArtifactRetentionIdentity) -> Option<&TerminalEvent> {
        if identity.run_id.is_none()
            && let Some(lease) = identity.lease_id
        {
            return self
                .released_leases
                .get(&(identity.owner_epoch, identity.instance_id, lease));
        }
        self.closures
            .get(&ClosureScope::from_identity(identity))?
            .close
            .as_ref()
    }

    pub(super) fn from_events<E: LedgerEventRead>(events: &[E]) -> GlobalLedgerResult<Self> {
        Self::from_events_checked(events, &mut |_| Ok(()))
    }

    fn from_events_checked<E: LedgerEventRead>(
        events: &[E],
        check: &mut impl FnMut(usize) -> GlobalLedgerResult<()>,
    ) -> GlobalLedgerResult<Self> {
        let mut retained = Self::default();
        let mut indexes = EventIndexes::default();
        for (position, event) in events.iter().enumerate() {
            check(position + 1)?;
            retained.validate(event, &events[..position], &indexes, true)?;
            retained.apply(event);
            indexes.insert(event, position);
        }
        Ok(retained)
    }

    /// Material metadata keeps its historical identity even after the bytes are unavailable.
    pub(super) fn proof(
        &self,
        reference: &ProjectedArtifactReference,
    ) -> GlobalLedgerResult<Option<ArtifactEvictionProof>> {
        let Some(object) = self.objects.get(&reference.artifact_id) else {
            return Ok(None);
        };
        if object.reference != *reference {
            return Err(invalid("artifact_identity_conflict"));
        }
        Ok(object.proof.clone().map(|mut proof| {
            proof.through_sequence = self.through_sequence;
            proof
        }))
    }

    pub(super) fn validate<E: LedgerEventRead>(
        &self,
        event: &E,
        events: &[E],
        indexes: &EventIndexes,
        guarded_intent: bool,
    ) -> GlobalLedgerResult<()> {
        if event.sequence() != self.through_sequence.saturating_add(1) {
            return Err(invalid("artifact_retention_snapshot_gap"));
        }
        // An outcome records the result of an already sealed identity, never a new use.
        let retention = event.payload().artifact_retention();
        for reference in material_references(event) {
            reference
                .validate()
                .map_err(|_| invalid("invalid_artifact_reference"))?;
            if let Some(object) = self.objects.get(&reference.artifact_id) {
                if object.reference != reference {
                    return Err(invalid("artifact_identity_conflict"));
                }
                if object.proof.is_some() {
                    return Err(sealed());
                }
            }
        }
        if retention.is_none() {
            for id in self.protection_targets(event) {
                if self
                    .objects
                    .get(&id)
                    .is_some_and(|object| object.proof.is_some())
                {
                    return Err(sealed());
                }
            }
        }
        if retention.is_none() {
            for frame in referenced_frames(event) {
                if self.frames.get(&frame).is_some_and(|ids| {
                    ids.iter().any(|id| {
                        self.objects
                            .get(id)
                            .is_some_and(|object| object.proof.is_some())
                    })
                }) {
                    return Err(sealed());
                }
            }
            let lab = event.origin().source() == EventSource::Lab
                || event.event_type() == EventType::LabRequest
                || indexes.lab_related(event, self.through_sequence);
            if lab
                && scopes(event).any(|scope| {
                    self.sealed_scopes.contains(&scope)
                        || self.linked_runs.get(&scope).is_some_and(|runs| {
                            runs.iter()
                                .any(|run| self.sealed_scopes.contains(&RetentionScope::Run(*run)))
                        })
                })
            {
                return Err(sealed());
            }
        }
        let Some(fact) = retention else { return Ok(()) };
        fact.validate()
            .map_err(|_| invalid("invalid_artifact_retention_fact"))?;
        let identity = fact.identity();
        if !same_links(event, identity) {
            return Err(invalid("artifact_retention_links_conflict"));
        }
        let object = self
            .objects
            .get(&identity.artifact.artifact_id)
            .ok_or_else(|| invalid("artifact_retention_source_missing"))?;
        if object.reference != identity.artifact
            || object
                .identity
                .as_ref()
                .is_some_and(|original| original != identity)
        {
            return Err(invalid("artifact_retention_identity_conflict"));
        }
        match fact {
            ArtifactRetentionFact::PinRecorded(pin) => {
                if object.proof.is_some() {
                    return Err(sealed());
                }
                let trigger = source(events, &pin.trigger)?;
                if !same_scope(trigger, identity)
                    || self.owner_at(trigger.sequence()) != Some(identity.owner_epoch)
                    || !material_references(trigger)
                        .iter()
                        .any(|value| value == &object.reference)
                        && trigger.links().frame_id() != object.reference.frame_id.as_ref()
                        && pin.reason != ArtifactPinReason::WarningOrHigher
                        && pin.reason != ArtifactPinReason::Lab
                {
                    return Err(invalid("artifact_pin_trigger_conflict"));
                }
                if pin.reason == ArtifactPinReason::WarningOrHigher
                    && trigger.severity() < EventSeverity::Warning
                    || pin.reason == ArtifactPinReason::Lab
                        && !indexes.lab_related(trigger, self.through_sequence)
                {
                    return Err(invalid("artifact_pin_reason_conflict"));
                }
            }
            ArtifactRetentionFact::PinReleased(release) => {
                if object.proof.is_some() {
                    return Err(sealed());
                }
                let Some((pin, reason)) = object.pins.get(&release.pin.event_id) else {
                    return Err(invalid("artifact_pin_release_source_missing"));
                };
                // Only a temporary runtime pin has a release path. Durable evidence pins remain.
                if *pin != release.pin || *reason != ArtifactPinReason::Explicit {
                    return Err(invalid("artifact_pin_not_releasable"));
                }
                let close = source(events, &release.release)?;
                if !same_close_scope(close, identity)
                    || !successful_close(close, identity)
                    || self.owner_at(close.sequence()) != Some(identity.owner_epoch)
                {
                    return Err(invalid("artifact_pin_release_not_closed"));
                }
            }
            ArtifactRetentionFact::EvictionIntent(intent) => {
                if !guarded_intent {
                    return Err(invalid("artifact_eviction_guard_required"));
                }
                self.validate_intent(object, intent, events, indexes)?;
            }
            ArtifactRetentionFact::EvictionOutcome(outcome) => {
                if !guarded_intent {
                    return Err(invalid("artifact_eviction_guard_required"));
                }
                let proof = object
                    .proof
                    .as_ref()
                    .ok_or_else(|| invalid("artifact_eviction_intent_missing"))?;
                if proof.intent != outcome.intent || proof.outcome.is_some() {
                    return Err(invalid("artifact_eviction_outcome_conflict"));
                }
            }
        }
        Ok(())
    }

    fn validate_intent<E: LedgerEventRead>(
        &self,
        object: &RetainedObject,
        intent: &ArtifactEvictionIntentRecord,
        events: &[E],
        indexes: &EventIndexes,
    ) -> GlobalLedgerResult<()> {
        if intent.through_sequence != self.through_sequence
            || object.proof.is_some()
            || object.permanently_protected
            || self.unlinked_warning
            || !object.pins.is_empty()
            || object.identity.as_ref() != Some(&intent.identity)
            || object.verified.as_ref() != Some(&intent.verified)
        {
            return Err(invalid("artifact_eviction_not_eligible"));
        }
        let verified = source(events, &intent.verified)?;
        let success = source(events, &intent.success)?;
        let close = source(events, &intent.close)?;
        if !same_scope(verified, &intent.identity)
            || !same_scope(success, &intent.identity)
            || !same_close_scope(close, &intent.identity)
            || indexes.lab_related(verified, self.through_sequence)
            || [verified, success, close]
                .iter()
                .any(|event| self.owner_at(event.sequence()) != Some(intent.identity.owner_epoch))
            || !successful_close(close, &intent.identity)
        {
            return Err(invalid("artifact_eviction_close_conflict"));
        }
        match intent.identity.run_id {
            Some(_) => {
                if !matches!(success.payload(), EventPayload::Task(TaskPayload::Semantic(payload))
                    if matches!(payload.fact(), TaskSemanticFact::TerminalCommitted { outcome: TaskOutcome::Success, .. }))
                {
                    return Err(invalid("artifact_eviction_task_not_successful"));
                }
                let summary_ref = intent
                    .capture_summary
                    .as_ref()
                    .ok_or_else(|| invalid("artifact_eviction_summary_missing"))?;
                let summary = source(events, summary_ref)?;
                if !same_scope(summary, &intent.identity)
                    || summary.sequence() >= close.sequence()
                    || !matches!(summary.payload(), EventPayload::Capture(CapturePayload::SummaryCommitted(payload))
                        if payload.summary().frames().iter().any(|frame| frame.artifact() == &object.reference))
                {
                    return Err(invalid("artifact_eviction_summary_conflict"));
                }
            }
            None if success.event_type() == EventType::InputCommitted
                && success.payload().effect_disposition() == Some(EffectDisposition::Performed) => {
            }
            None if intent.identity.lease_id.is_none()
                && success.event_type() == EventType::CaptureCompleted => {}
            None => return Err(invalid("artifact_eviction_success_missing")),
        }
        let scheduled = intent
            .identity
            .run_id
            .is_some_and(|run| self.scheduled_runs.contains(&run));
        match (&intent.settlement, scheduled) {
            (Some(settlement), true) => {
                let settlement = source(events, settlement)?;
                if !same_scope(settlement, &intent.identity)
                    || settlement.sequence() <= success.sequence()
                    || self.owner_at(settlement.sequence()) != Some(intent.identity.owner_epoch)
                    || !matches!(settlement.payload(), EventPayload::Policy(PolicyPayload::ExecutionRecorded(payload))
                        if matches!(payload.outcome(), PolicyExecutionOutcome::Succeeded { .. }))
                {
                    return Err(invalid("artifact_eviction_settlement_conflict"));
                }
            }
            (None, false) => {}
            _ => return Err(invalid("artifact_eviction_settlement_missing")),
        }
        Ok(())
    }

    pub(super) fn apply<E: LedgerEventRead>(&mut self, event: &E) {
        if let Some(epoch) = recorded_owner(event) {
            self.owner_epochs.insert(event.sequence(), epoch);
        }
        if event.links().run_id().is_none()
            && let (Some(owner), Some(instance), Some(lease)) = (
                self.owner_at(event.sequence()),
                event.links().instance_id(),
                event.links().lease_id(),
            )
            && matches!(event.payload(), EventPayload::Lease(LeasePayload::Released(payload))
                if payload.effect_disposition() == EffectDisposition::Performed)
        {
            self.released_leases
                .insert((owner, *instance, *lease), terminal(event));
        }
        if let Some(scope) = self
            .owner_at(event.sequence())
            .and_then(|owner| ClosureScope::from_event(event, owner))
        {
            let closure = self.closures.entry(scope).or_default();
            let successful = match event.payload() {
                EventPayload::Task(TaskPayload::Semantic(payload)) => matches!(
                    payload.fact(),
                    TaskSemanticFact::TerminalCommitted {
                        outcome: TaskOutcome::Success,
                        ..
                    }
                ),
                _ => {
                    scope.run.is_none()
                        && (event.event_type() == EventType::InputCommitted
                            && event.payload().effect_disposition()
                                == Some(EffectDisposition::Performed)
                            || scope.lease.is_none()
                                && event.event_type() == EventType::CaptureCompleted)
                }
            };
            if successful {
                closure.success = Some(terminal(event));
            }
            if matches!(event.payload(), EventPayload::Lease(LeasePayload::Released(payload))
                if payload.effect_disposition() == EffectDisposition::Performed)
                || matches!(event.payload(), EventPayload::Runtime(RuntimePayload::LifecycleObserved(payload))
                    if matches!(payload.phase(), RuntimeLifecyclePhase::ResourceQuiescence {
                        instance_id, resource_count, quiescence: actingcommand_contract::ResourceQuiescence::Confirmed,
                        owner_disposition: actingcommand_contract::OwnerResourceDisposition::ConfirmedClosed,
                    } if instance_id == scope.instance && resource_count > 0))
            {
                closure.close = Some(terminal(event));
            }
            if matches!(event.payload(), EventPayload::Policy(PolicyPayload::ExecutionRecorded(payload))
                if matches!(payload.outcome(), PolicyExecutionOutcome::Succeeded { .. }))
            {
                closure.settlement = Some(terminal(event));
            }
        }
        if let Some(run) = event.links().run_id() {
            for scope in scopes(event) {
                self.linked_runs.entry(scope).or_default().insert(*run);
            }
            if matches!(
                event.event_type(),
                EventType::PolicyDispatchIntent | EventType::PolicyDispatchAdmitted
            ) {
                self.scheduled_runs.insert(*run);
            }
        }
        if event.payload().artifact_retention().is_none() {
            if event.severity() >= EventSeverity::Warning && scopes(event).next().is_none() {
                self.unlinked_warning = true;
            }
            for id in self.protection_targets(event) {
                if let Some(object) = self.objects.get_mut(&id) {
                    object.permanently_protected = true;
                }
            }
        }
        let mut observed_frames = material_references(event)
            .into_iter()
            .filter_map(|reference| reference.frame_id)
            .collect::<Vec<_>>();
        if event.event_type() == EventType::CaptureCompleted {
            observed_frames.extend(event.links().frame_id().copied());
        }
        for frame in observed_frames {
            if !self.seen_frames.insert(frame) {
                continue;
            }
            for scope in scopes(event) {
                let recent = self.recent.entry(scope).or_default();
                recent.push_back(frame);
                if recent.len() > FRAME_RETENTION_BACKTRACE {
                    recent.pop_front();
                }
            }
        }
        for reference in material_references(event) {
            if reference.kind != ArtifactKind::CaptureFrame {
                continue;
            }
            if let Some(frame) = reference.frame_id {
                self.frames
                    .entry(frame)
                    .or_default()
                    .insert(reference.artifact_id);
            }
            let protect = event.severity() >= EventSeverity::Warning
                || direct_evidence(event).contains(&reference.artifact_id);
            let object = self
                .objects
                .entry(reference.artifact_id)
                .or_insert_with(|| RetainedObject {
                    reference,
                    verified: None,
                    summary: None,
                    identity: None,
                    pins: BTreeMap::new(),
                    permanently_protected: protect,
                    proof: None,
                });
            object.permanently_protected |= protect;
            if event.event_type() == EventType::ArtifactVerified {
                object.verified.get_or_insert_with(|| terminal(event));
            }
            if event.event_type() == EventType::CaptureSummaryCommitted {
                object.summary = Some(terminal(event));
            }
        }
        if let Some(fact) = event.payload().artifact_retention() {
            let object = self
                .objects
                .get_mut(&fact.identity().artifact.artifact_id)
                .expect("retention source validated before commit");
            object
                .identity
                .get_or_insert_with(|| fact.identity().clone());
            match fact {
                ArtifactRetentionFact::PinRecorded(pin) => {
                    object
                        .pins
                        .insert(*event.event_id(), (terminal(event), pin.reason));
                }
                ArtifactRetentionFact::PinReleased(release) => {
                    object.pins.remove(&release.pin.event_id);
                }
                ArtifactRetentionFact::EvictionIntent(_) => {
                    self.pending_objects
                        .insert(fact.identity().artifact.artifact_id);
                    self.sealed_scopes.extend(scopes(event));
                    object.proof = Some(ArtifactEvictionProof {
                        identity: fact.identity().clone(),
                        intent: terminal(event),
                        outcome: None,
                        disposition: None,
                        through_sequence: event.sequence(),
                    });
                }
                ArtifactRetentionFact::EvictionOutcome(outcome) => {
                    self.pending_objects
                        .remove(&fact.identity().artifact.artifact_id);
                    let proof = object
                        .proof
                        .as_mut()
                        .expect("intent validated before commit");
                    proof.outcome = Some(terminal(event));
                    proof.disposition = Some(outcome.disposition);
                }
            }
        }
        self.through_sequence = event.sequence();
    }

    fn owner_at(&self, sequence: u64) -> Option<OwnerEpoch> {
        self.owner_epochs
            .range(..=sequence)
            .next_back()
            .map(|(_, epoch)| *epoch)
    }

    /// The per-scope ring counts exact first-observed frames and never scans history.
    fn protection_targets<E: LedgerEventRead>(&self, event: &E) -> BTreeSet<ArtifactId> {
        let mut targets = direct_evidence(event);
        if let EventPayload::Input(actingcommand_contract::InputPayload::Intent(input)) =
            event.payload()
            && let Some(frame) = input
                .provenance()
                .and_then(|provenance| provenance.before_frame_id)
            && let Some(ids) = self.frames.get(&frame)
        {
            targets.extend(ids);
        }
        if event.severity() >= EventSeverity::Warning {
            targets.extend(
                material_references(event)
                    .iter()
                    .map(|reference| reference.artifact_id),
            );
            for frame in referenced_frames(event) {
                if let Some(ids) = self.frames.get(&frame) {
                    targets.extend(ids)
                }
            }
            let axes = if let Some(run) = event.links().run_id() {
                vec![RetentionScope::Run(*run)]
            } else {
                scopes(event).collect()
            };
            for scope in axes {
                if let Some(recent) = self.recent.get(&scope) {
                    for frame in recent {
                        if let Some(ids) = self.frames.get(frame) {
                            targets.extend(ids)
                        }
                    }
                }
            }
        }
        targets
    }
}

fn referenced_frames<E: LedgerEventRead>(event: &E) -> BTreeSet<FrameId> {
    let mut frames = event
        .links()
        .frame_id()
        .copied()
        .into_iter()
        .collect::<BTreeSet<_>>();
    if let EventPayload::Input(actingcommand_contract::InputPayload::Intent(input)) =
        event.payload()
    {
        frames.extend(
            input
                .provenance()
                .and_then(|provenance| provenance.before_frame_id),
        );
    }
    if let EventPayload::Capture(CapturePayload::DedupWindow(window)) = event.payload() {
        frames.extend(window.preserved_frame_id().copied());
    }
    frames
}

fn material_references<E: LedgerEventRead>(event: &E) -> Vec<ProjectedArtifactReference> {
    let mut references = event.projected_artifacts(true);
    if let EventPayload::Capture(CapturePayload::SummaryCommitted(payload)) = event.payload() {
        references.extend(
            payload
                .summary()
                .frames()
                .iter()
                .map(|frame| frame.artifact().clone()),
        );
        references.extend(
            payload
                .summary()
                .pinned()
                .iter()
                .filter_map(|pin| pin.artifact().cloned()),
        );
    }
    if let EventPayload::Fact(FactPayload::Published(payload)) = event.payload() {
        for record in payload.records() {
            if let FactContent::Artifact { artifact } = &record.content {
                references.push(artifact.clone());
            }
        }
    }
    references
}

fn direct_evidence<E: LedgerEventRead>(event: &E) -> BTreeSet<ArtifactId> {
    let mut ids = BTreeSet::new();
    match event.payload() {
        EventPayload::Capture(CapturePayload::SummaryCommitted(payload)) => {
            ids.extend(
                payload
                    .summary()
                    .pinned()
                    .iter()
                    .filter_map(|pin| pin.artifact())
                    .map(|reference| reference.artifact_id),
            );
        }
        EventPayload::Fact(FactPayload::Published(payload)) => {
            for record in payload.records() {
                if let FactContent::Artifact { artifact } = &record.content {
                    ids.insert(artifact.artifact_id);
                }
            }
        }
        _ => {}
    }
    ids
}

fn source<'a, E: LedgerEventRead>(
    events: &'a [E],
    reference: &TerminalEvent,
) -> GlobalLedgerResult<&'a E> {
    let position = events.partition_point(|event| event.sequence() < reference.sequence);
    events
        .get(position)
        .filter(|event| {
            event.sequence() == reference.sequence && event.event_id() == &reference.event_id
        })
        .ok_or_else(|| invalid("artifact_retention_proof_source_missing"))
}

fn terminal<E: LedgerEventRead>(event: &E) -> TerminalEvent {
    TerminalEvent {
        event_id: *event.event_id(),
        sequence: event.sequence(),
    }
}

fn same_scope<E: LedgerEventRead>(event: &E, identity: &ArtifactRetentionIdentity) -> bool {
    let links = event.links();
    links.instance_id() == Some(&identity.instance_id)
        && links.run_id() == identity.run_id.as_ref()
        && links.lease_id() == identity.lease_id.as_ref()
        && (identity.run_id.is_some()
            || links.request_id() == Some(&identity.request_id)
                && links.correlation_id() == Some(&identity.correlation_id))
}

fn same_links<E: LedgerEventRead>(event: &E, identity: &ArtifactRetentionIdentity) -> bool {
    same_scope(event, identity)
        && event.links().frame_id() == identity.artifact.frame_id.as_ref()
        && event.links().request_id() == Some(&identity.request_id)
        && event.links().correlation_id() == Some(&identity.correlation_id)
}

fn same_close_scope<E: LedgerEventRead>(event: &E, identity: &ArtifactRetentionIdentity) -> bool {
    if identity.run_id.is_none() && identity.lease_id.is_some() {
        return event.links().instance_id() == Some(&identity.instance_id)
            && event.links().lease_id() == identity.lease_id.as_ref()
            && event.links().run_id().is_none();
    }
    same_scope(event, identity)
}

fn successful_close<E: LedgerEventRead>(event: &E, identity: &ArtifactRetentionIdentity) -> bool {
    match (identity.lease_id, event.payload()) {
        (Some(_), EventPayload::Lease(LeasePayload::Released(payload))) => {
            payload.effect_disposition() == EffectDisposition::Performed
        }
        (None, EventPayload::Runtime(RuntimePayload::LifecycleObserved(payload))) => {
            payload.owner_epoch() == identity.owner_epoch
                && matches!(payload.phase(), RuntimeLifecyclePhase::ResourceQuiescence {
                instance_id, resource_count, quiescence: actingcommand_contract::ResourceQuiescence::Confirmed,
                owner_disposition: actingcommand_contract::OwnerResourceDisposition::ConfirmedClosed,
            } if instance_id == identity.instance_id && resource_count > 0)
        }
        _ => false,
    }
}

fn scopes<E: LedgerEventRead>(event: &E) -> impl Iterator<Item = RetentionScope> {
    [
        event.links().run_id().copied().map(RetentionScope::Run),
        event
            .links()
            .request_id()
            .copied()
            .map(RetentionScope::Request),
        event
            .links()
            .correlation_id()
            .copied()
            .map(RetentionScope::Correlation),
    ]
    .into_iter()
    .flatten()
}

fn recorded_owner<E: LedgerEventRead>(event: &E) -> Option<OwnerEpoch> {
    if let Some(fact) = event.payload().runtime_state() {
        return Some(match fact {
            RuntimeStateFact::Observed { state, .. } => state.owner_epoch(),
            RuntimeStateFact::MonitorImported { owner_epoch, .. }
            | RuntimeStateFact::MonitorChanged { owner_epoch, .. } => *owner_epoch,
        });
    }
    if let EventPayload::Runtime(RuntimePayload::LifecycleObserved(payload)) = event.payload() {
        return Some(payload.owner_epoch());
    }
    None
}

fn invalid(code: &'static str) -> GlobalLedgerError {
    GlobalLedgerError::fatal(code, "artifact_retention")
}

impl<B: super::storage::DurableStorage> super::storage::EventStore<B> {
    pub(super) fn retention_candidates(
        &self,
        after: Option<ArtifactId>,
    ) -> ArtifactRetentionCandidates {
        self.retention.candidates(after, self.recovering_retention)
    }

    /// The material guard arrived before this command; the writer performs no material I/O.
    pub(super) fn admit_artifact_eviction(
        &mut self,
        guard: ArtifactDeleteGuard,
    ) -> GlobalLedgerResult<(ArtifactEvictionAdmission, Vec<PersistedEvent>)> {
        if guard.root() != self.backend.material_root() {
            return Err(invalid("artifact_eviction_root_conflict"));
        }
        let reference = guard.reference().clone();
        if let Some(proof) = self.retention.proof(&reference)? {
            if proof.outcome.is_some() {
                return Ok((ArtifactEvictionAdmission::Deferred, Vec::new()));
            }
            if !self.recovering_retention {
                return Err(GlobalLedgerError::request(
                    "artifact_eviction_intent_pending",
                    "admit_artifact_eviction",
                ));
            }
            let original = source(&self.events, &proof.intent)?;
            let Some(ArtifactRetentionFact::EvictionIntent(intent)) =
                original.payload().artifact_retention()
            else {
                return Err(invalid("artifact_eviction_intent_invalid"));
            };
            return Ok((
                ArtifactEvictionAdmission::Committed(Box::new(ArtifactEvictionPermit {
                    guard,
                    intent: intent.clone(),
                    source: proof.intent,
                    recovery: true,
                })),
                Vec::new(),
            ));
        }
        if self.recovering_retention {
            return Ok((ArtifactEvictionAdmission::Deferred, Vec::new()));
        }
        let Some(object) = self.retention.objects.get(&reference.artifact_id) else {
            return Ok((ArtifactEvictionAdmission::Deferred, Vec::new()));
        };
        let (Some(identity), Some(verified)) = (&object.identity, &object.verified) else {
            return Ok((ArtifactEvictionAdmission::Deferred, Vec::new()));
        };
        let identity = identity.clone();
        let verified = verified.clone();
        let releases = self
            .retention
            .close_for(&identity)
            .map(|close| {
                object
                    .pins
                    .values()
                    .filter(|(_, reason)| *reason == ArtifactPinReason::Explicit)
                    .take(RETENTION_ROUND_OBJECTS)
                    .map(
                        |(pin, _)| actingcommand_contract::ArtifactPinReleaseRecord {
                            identity: identity.clone(),
                            pin: pin.clone(),
                            release: close.clone(),
                        },
                    )
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut appended = Vec::new();
        for release in releases {
            appended.push(self.append_retention_fact(
                ArtifactRetentionFact::PinReleased(release),
                &verified,
                false,
            )?);
        }
        let Some(intent) = self
            .retention
            .intent(&reference, &self.events, &self.indexes)?
        else {
            return Ok((ArtifactEvictionAdmission::Deferred, appended));
        };
        if !guard.material_present() {
            return Err(invalid("artifact_store_verification_failed"));
        }
        let event = self.append_retention_fact(
            ArtifactRetentionFact::EvictionIntent(intent.clone()),
            &verified,
            true,
        )?;
        let source = terminal(&event);
        appended.push(event);
        Ok((
            ArtifactEvictionAdmission::Committed(Box::new(ArtifactEvictionPermit {
                guard,
                intent,
                source,
                recovery: false,
            })),
            appended,
        ))
    }

    pub(super) fn finish_artifact_eviction(
        &mut self,
        permit: ArtifactEvictionPermit,
        disposition: ArtifactEvictionDisposition,
        io: Option<actingcommand_contract::ArtifactEvictionIo>,
    ) -> GlobalLedgerResult<PersistedEvent> {
        if permit.guard.root() != self.backend.material_root()
            || permit.guard.reference() != &permit.intent.identity.artifact
            || disposition == ArtifactEvictionDisposition::RecoveryAbsent
                && (!permit.recovery || permit.guard.material_present())
        {
            return Err(invalid("artifact_eviction_outcome_guard_conflict"));
        }
        let fact = ArtifactRetentionFact::EvictionOutcome(
            actingcommand_contract::ArtifactEvictionOutcomeRecord {
                identity: permit.intent.identity,
                intent: permit.source.clone(),
                disposition,
                io,
            },
        );
        let event = self.append_retention_fact(fact, &permit.source, true)?;
        self.recovering_retention &= self.retention.has_pending();
        // `permit.guard` stays alive through the durable outcome and in-memory admission update.
        drop(permit.guard);
        Ok(event)
    }

    fn append_retention_fact(
        &mut self,
        fact: ArtifactRetentionFact,
        source_ref: &TerminalEvent,
        guarded: bool,
    ) -> GlobalLedgerResult<PersistedEvent> {
        use actingcommand_contract::{
            ArtifactPayloadDraft, AuditInput, EventActor, EventDraft, EventLinksDraft, EventOrigin,
            IdentifierIssuer, OriginModule,
        };
        let links = source(&self.events, source_ref)?
            .links()
            .artifact_retention_source();
        let severity = if matches!(&fact, ArtifactRetentionFact::EvictionOutcome(outcome)
            if outcome.disposition == ArtifactEvictionDisposition::Failed)
        {
            EventSeverity::Error
        } else {
            EventSeverity::Info
        };
        let ids =
            IdentifierIssuer::new().map_err(|_| invalid("artifact_retention_identifier_failed"))?;
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|time| u64::try_from(time.as_millis()).ok())
            .ok_or_else(|| invalid("artifact_retention_clock_failed"))?;
        let draft = EventDraft::new(
            ids.mint_event_id()
                .map_err(|_| invalid("artifact_retention_identifier_failed"))?,
            timestamp,
            severity,
            EventOrigin::new(
                EventSource::Runtime,
                OriginModule::ArtifactStore,
                EventActor::Runtime,
            ),
            EventLinksDraft::default(),
            ArtifactPayloadDraft::retention(fact, AuditInput::new()).into(),
        )
        .sanitize(&super::Sha256SecretFingerprinter::new(
            b"actingcommand-ledger-artifact-retention-v1",
        )?)
        .map_err(|_| invalid("artifact_retention_sanitize_failed"))?;
        let draft = links
            .apply_to(draft)
            .map_err(|error| invalid(error.code()))?;
        let event = PersistedEvent::from_sanitized(self.next_sequence, draft)
            .map_err(|error| invalid(error.code()))?;
        self.persist_retention_checked(event, guarded)
    }
}

impl ArtifactEvictionPermit {
    /// Material I/O occurs in the Runtime caller, outside the Ledger writer.
    pub(super) fn perform(
        &mut self,
    ) -> (
        ArtifactEvictionDisposition,
        Option<actingcommand_contract::ArtifactEvictionIo>,
    ) {
        if self.recovery && !self.guard.material_present() {
            return (ArtifactEvictionDisposition::RecoveryAbsent, None);
        }
        match self.guard.remove_after_durable_intent(&self.intent) {
            Ok(()) => (ArtifactEvictionDisposition::Deleted, None),
            Err(error) => (
                ArtifactEvictionDisposition::Failed,
                Some(actingcommand_contract::ArtifactEvictionIo::from_io(&error)),
            ),
        }
    }
}

impl super::GlobalLedger {
    pub fn retention_candidates(
        &self,
        after: Option<ArtifactId>,
    ) -> GlobalLedgerResult<ArtifactRetentionCandidates> {
        let (response, receiver) = std::sync::mpsc::sync_channel(1);
        let sender = self
            .sender
            .as_ref()
            .ok_or_else(|| invalid("writer_unavailable"))?;
        super::send_command(
            sender,
            super::WriterCommand::RetentionCandidates { after, response },
            "retention_candidates",
        )?;
        super::receive_response(receiver, "retention_candidates")?
    }

    pub fn admit_artifact_eviction(
        &self,
        guard: ArtifactDeleteGuard,
    ) -> GlobalLedgerResult<ArtifactEvictionAdmission> {
        let (response, receiver) = std::sync::mpsc::sync_channel(1);
        let sender = self
            .sender
            .as_ref()
            .ok_or_else(|| invalid("writer_unavailable"))?;
        super::send_command(
            sender,
            super::WriterCommand::AdmitArtifactEviction {
                guard: Box::new(guard),
                response,
            },
            "admit_artifact_eviction",
        )?;
        super::receive_response(receiver, "admit_artifact_eviction")?
    }

    pub fn finish_artifact_eviction(
        &self,
        mut permit: Box<ArtifactEvictionPermit>,
    ) -> GlobalLedgerResult<PersistedEvent> {
        self.check_writer_health()?;
        let sender = self
            .sender
            .as_ref()
            .ok_or_else(|| invalid("writer_unavailable"))?;
        let (disposition, io) = permit.perform();
        let (response, receiver) = std::sync::mpsc::sync_channel(1);
        super::send_command(
            sender,
            super::WriterCommand::FinishArtifactEviction {
                permit,
                disposition,
                io,
                response,
            },
            "finish_artifact_eviction",
        )?;
        super::receive_response(receiver, "finish_artifact_eviction")?
    }
}

fn sealed() -> GlobalLedgerError {
    GlobalLedgerError::request("artifact_material_admission_closed", "append_event")
}

pub(super) fn annotate_metadata_checked(
    events: &mut [crate::fact::LedgerEventMetadata],
    mut check: impl FnMut(usize) -> GlobalLedgerResult<()>,
) -> GlobalLedgerResult<()> {
    let retention = RetentionIndex::from_events_checked(events, &mut check)?;
    for (position, event) in events.iter_mut().enumerate() {
        check(position + 1)?;
        event.apply_artifact_evictions(retention.event_proofs(event));
    }
    Ok(())
}

/// Authentication and typed proof derivation precede every material read in the same snapshot.
pub(super) fn restore_records<F>(
    records: Vec<StoredEventRecord>,
    verifier: &mut Option<F>,
    mut check: impl FnMut(usize) -> GlobalLedgerResult<()>,
) -> GlobalLedgerResult<Vec<PersistedEvent>>
where
    F: FnMut(&ProjectedArtifactReference) -> Option<VerifiedArtifactReference>,
{
    let mut metadata = Vec::with_capacity(records.len());
    for record in &records {
        check(metadata.len() + 1)?;
        metadata.push(
            record
                .clone()
                .into_metadata()
                .map_err(|error| invalid(error.code()))?,
        );
    }
    let retention = RetentionIndex::from_events_checked(&metadata, &mut check)?;
    let mut events = Vec::with_capacity(records.len());
    for record in records {
        check(events.len() + 1)?;
        let mut event = record
            .into_event_with_artifact_availability(&mut |reference| {
                let proof = retention
                    .proof(reference)
                    .map_err(|error| FactValidationError::new(error.code()))?;
                if let Some(proof) = proof {
                    return match proof.disposition {
                        Some(
                            ArtifactEvictionDisposition::Deleted
                            | ArtifactEvictionDisposition::RecoveryAbsent,
                        ) => Ok(ArtifactAvailability::Evicted(Box::new(proof))),
                        None => Ok(ArtifactAvailability::PendingEviction(Box::new(proof))),
                        Some(ArtifactEvictionDisposition::Failed) => {
                            Err(FactValidationError::new("artifact_eviction_failed"))
                        }
                    };
                }
                let verify = verifier.as_mut().ok_or(FactValidationError::new(
                    "artifact_store_verification_unavailable",
                ))?;
                verify(reference)
                    .map(ArtifactAvailability::Available)
                    .ok_or(FactValidationError::new(
                        "artifact_store_verification_failed",
                    ))
            })
            .map_err(|error| invalid(error.code()))?;
        retention.annotate_event(&mut event);
        check(events.len() + 1)?;
        events.push(event);
    }
    Ok(events)
}
