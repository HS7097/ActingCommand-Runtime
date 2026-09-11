// SPDX-License-Identifier: AGPL-3.0-only

use super::{GlobalLedgerError, GlobalLedgerResult, projection::EventIndexes};
use crate::fact::LedgerEventRead;
use actingcommand_contract::{
    ArtifactEvictionDisposition, ArtifactEvictionIntentRecord, ArtifactEvictionProof, ArtifactId,
    ArtifactKind, ArtifactPinReason, ArtifactRetentionFact, ArtifactRetentionIdentity,
    CapturePayload, EffectDisposition, EventId, EventPayload, EventSeverity, EventType,
    FRAME_RETENTION_BACKTRACE, FrameId, LeasePayload, PolicyExecutionOutcome, PolicyPayload,
    ProjectedArtifactReference, RuntimeLifecyclePhase, RuntimePayload, TaskOutcome, TaskPayload,
    TaskSemanticFact, TerminalEvent,
};
use std::collections::{BTreeMap, BTreeSet};

/// Derived only from the authenticated prefix, inside the original Ledger owner.
#[derive(Default)]
pub(super) struct RetentionIndex {
    objects: BTreeMap<ArtifactId, RetainedObject>,
    frames: BTreeMap<FrameId, BTreeSet<ArtifactId>>,
    through_sequence: u64,
}

struct RetainedObject {
    reference: ProjectedArtifactReference,
    verified: Option<TerminalEvent>,
    identity: Option<ArtifactRetentionIdentity>,
    pins: BTreeMap<EventId, (TerminalEvent, ArtifactPinReason)>,
    permanently_protected: bool,
    proof: Option<ArtifactEvictionProof>,
}

impl RetentionIndex {
    pub(super) fn from_events<E: LedgerEventRead>(events: &[E]) -> GlobalLedgerResult<Self> {
        let mut retained = Self::default();
        let mut indexes = EventIndexes::default();
        for (position, event) in events.iter().enumerate() {
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
            for id in self.protection_targets(event, events) {
                if self
                    .objects
                    .get(&id)
                    .is_some_and(|object| object.proof.is_some())
                {
                    return Err(sealed());
                }
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
                if !same_scope(close, identity) || !successful_close(close, identity) {
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
            || !same_scope(close, &intent.identity)
            || indexes.lab_related(verified, self.through_sequence)
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
        let scheduled = events.iter().any(|event| {
            event.links().run_id() == intent.identity.run_id.as_ref()
                && intent.identity.run_id.is_some()
                && event.event_type() == EventType::PolicyDispatchIntent
        });
        match (&intent.settlement, scheduled) {
            (Some(settlement), true) => {
                let settlement = source(events, settlement)?;
                if !same_scope(settlement, &intent.identity)
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
            let object = self
                .objects
                .entry(reference.artifact_id)
                .or_insert_with(|| RetainedObject {
                    reference,
                    verified: None,
                    identity: None,
                    pins: BTreeMap::new(),
                    permanently_protected: false,
                    proof: None,
                });
            if event.event_type() == EventType::ArtifactVerified {
                object.verified.get_or_insert_with(|| terminal(event));
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
                    object.proof = Some(ArtifactEvictionProof {
                        identity: fact.identity().clone(),
                        intent: terminal(event),
                        outcome: None,
                        disposition: None,
                        through_sequence: event.sequence(),
                    });
                }
                ArtifactRetentionFact::EvictionOutcome(outcome) => {
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

    /// Warning evidence includes the exact recent frame identities, not approximate representatives.
    fn protection_targets<E: LedgerEventRead>(
        &self,
        event: &E,
        events: &[E],
    ) -> BTreeSet<ArtifactId> {
        let mut targets = BTreeSet::new();
        if let Some(frame) = event.links().frame_id() {
            if let Some(ids) = self.frames.get(frame) {
                targets.extend(ids)
            }
        }
        if event.severity() >= EventSeverity::Warning {
            let mut frames = BTreeSet::new();
            for previous in events
                .iter()
                .rev()
                .filter(|previous| associated(previous, event))
            {
                for reference in material_references(previous) {
                    if reference.kind == ArtifactKind::CaptureFrame {
                        if let Some(frame) = reference.frame_id {
                            if frames.len() < FRAME_RETENTION_BACKTRACE || frames.contains(&frame) {
                                frames.insert(frame);
                                targets.insert(reference.artifact_id);
                            }
                        }
                    }
                }
                if frames.len() >= FRAME_RETENTION_BACKTRACE {
                    break;
                }
            }
        }
        targets
    }
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
    references
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
        && links.request_id() == Some(&identity.request_id)
        && links.correlation_id() == Some(&identity.correlation_id)
        && links.run_id() == identity.run_id.as_ref()
        && links.lease_id() == identity.lease_id.as_ref()
}

fn same_links<E: LedgerEventRead>(event: &E, identity: &ArtifactRetentionIdentity) -> bool {
    same_scope(event, identity) && event.links().frame_id() == identity.artifact.frame_id.as_ref()
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

fn associated<E: LedgerEventRead>(left: &E, right: &E) -> bool {
    let a = left.links();
    let b = right.links();
    if let Some(run) = b.run_id() {
        return a.run_id() == Some(run);
    }
    b.request_id().is_some() && a.request_id() == b.request_id()
        || b.correlation_id().is_some() && a.correlation_id() == b.correlation_id()
}

fn invalid(code: &'static str) -> GlobalLedgerError {
    GlobalLedgerError::fatal(code, "artifact_retention")
}

fn sealed() -> GlobalLedgerError {
    GlobalLedgerError::request("artifact_material_admission_closed", "append_event")
}
