// SPDX-License-Identifier: AGPL-3.0-only

use super::{GlobalLedgerError, GlobalLedgerResult, projection::EventIndexes};
use crate::codes::{LedgerCode, LedgerLocation};
use crate::fact::{FactValidationError, LedgerEventRead, StoredEventRecord};
use crate::{ArtifactAvailability, PersistedEvent};
use actingcommand_artifact_store::ArtifactDeleteGuard;
use actingcommand_contract::codes::{ContractLocation, SharedCode};
use actingcommand_contract::outcome::Code;
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
mod material;
mod prior_epoch;
mod view;
pub use material::LedgerOpenTiming;
pub(super) use material::{PendingMaterial, elapsed_ms};
pub use view::{
    FrameErrorPoint, FrameRetentionClass, FrameRetentionFrame, FrameRetentionSwitches,
    FrameRetentionView, KeptFrameFolder,
};

/// Derived only from the authenticated prefix, inside the original Ledger owner.
#[derive(Default)]
pub(super) struct RetentionIndex {
    objects: BTreeMap<ArtifactId, RetainedObject>,
    frames: BTreeMap<FrameId, BTreeSet<ArtifactId>>,
    recent: BTreeMap<RetentionScope, VecDeque<FrameId>>,
    seen_frames: BTreeSet<FrameId>,
    linked_runs: BTreeMap<RetentionScope, BTreeSet<RunId>>,
    run_scopes: BTreeMap<RunId, BTreeMap<RetentionScope, u64>>,
    lab_anchors: BTreeMap<RetentionScope, u64>,
    material_scope_activity: BTreeMap<RetentionScope, u64>,
    sealed_scopes: BTreeSet<RetentionScope>,
    scheduled_runs: BTreeSet<RunId>,
    run_terminals: BTreeMap<RunId, RunTerminal>,
    owner_epochs: BTreeMap<u64, OwnerEpoch>,
    owner_ends: BTreeMap<OwnerEpoch, u64>,
    reopened_owners: BTreeSet<OwnerEpoch>,
    scope_sources: BTreeMap<ClosureScope, (TerminalEvent, u64)>,
    owner_imports:
        BTreeMap<OwnerEpoch, (TerminalEvent, actingcommand_contract::PriorEpochOwnerImport)>,
    prior_epoch_closes:
        BTreeMap<ClosureScope, (TerminalEvent, actingcommand_contract::PriorEpochScopeClose)>,
    closures: BTreeMap<ClosureScope, ClosureFacts>,
    released_leases: BTreeMap<(OwnerEpoch, InstanceId, LeaseId), TerminalEvent>,
    quiescent_instances: BTreeMap<(OwnerEpoch, InstanceId), TerminalEvent>,
    pending_objects: BTreeSet<ArtifactId>,
    unlinked_warning: bool,
    /// Workflow #375 R5c: the facts of the read-only frame retention view.
    frame_facts: view::FrameFacts,
    /// Workflow #375 R5d: the frame classes at the latest head they were asked for.
    frame_cache: std::sync::Mutex<Option<view::FrameCache>>,
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
    last_reference_sequence: u64,
    verified: Option<TerminalEvent>,
    summary: Option<TerminalEvent>,
    identity: Option<ArtifactRetentionIdentity>,
    pins: BTreeMap<EventId, (TerminalEvent, ArtifactPinReason)>,
    released_lab_pins: BTreeMap<EventId, actingcommand_contract::LabPinReleaseResult>,
    reference_scopes: BTreeSet<RetentionScope>,
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

/// Workflow #382 L-5: what admission still asks of a run's terminal (one, in a known scope).
/// The K/T fields (outcome, time, ordered successes) left with the K/T admission.
struct RunTerminal {
    scope: Option<ClosureScope>,
    contradictory: bool,
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
    /// These snapshot observations only skip work; admission still validates the current writer state.
    pub ineligible: BTreeSet<ArtifactId>,
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
        let references: Vec<ProjectedArtifactReference> = if recovery_only {
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
        let ineligible = if recovery_only {
            BTreeSet::new()
        } else {
            references
                .iter()
                .filter_map(|reference| {
                    let object = self.objects.get(&reference.artifact_id)?;
                    let eligible_scope = object.identity.as_ref().is_some_and(|identity| {
                        self.run_is_unambiguous(identity)
                            && self
                                .closures
                                .get(&ClosureScope::from_identity(identity))
                                .is_some_and(|closure| closure.success.is_some())
                            && self.releasing_close(object, identity).is_some()
                    });
                    (self.unlinked_warning
                        || object.permanently_protected
                        || object
                            .pins
                            .values()
                            .any(|(_, reason)| *reason != ArtifactPinReason::Explicit)
                        || object.proof.is_some()
                        || object.verified.is_none()
                        || !eligible_scope)
                        .then_some(reference.artifact_id)
                })
                .collect()
        };
        ArtifactRetentionCandidates {
            references,
            ineligible,
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
            return Err(invalid(LedgerCode::ArtifactIdentityConflict.code()));
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
        if !self.run_is_unambiguous(identity) {
            return Ok(None);
        }
        if self.lab_protected(object, source(events, verified)?, indexes) {
            return Ok(None);
        }
        let Some(closure) = self.closures.get(&ClosureScope::from_identity(identity)) else {
            return Ok(None);
        };
        let Some(close) = self.close_for(identity) else {
            return Ok(None);
        };
        // Workflow #382 L-5: only a success sources an intent. The K/T branch, which let a failed
        // or cancelled run's frame qualify after later successes or elapsed days, is retired.
        let Some(success) = closure.success else {
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
        // A settlement that does not match the terminal branch (e.g. a success that overran its
        // runtime budget) keeps the object; it never reaches the fatal admission check.
        if let Some(settlement) = &closure.settlement
            && !settlement_matches(source(events, settlement)?, TaskOutcome::Success)
        {
            return Ok(None);
        }
        let intent = ArtifactEvictionIntentRecord {
            identity: identity.clone(),
            verified: *verified,
            success: Some(success),
            close: *close,
            capture_summary: object.summary,
            settlement: closure.settlement,
            through_sequence: self.through_sequence,
        };
        self.validate_intent(object, &intent, events, indexes, true)?;
        Ok(Some(intent))
    }

    fn run_is_unambiguous(&self, identity: &ArtifactRetentionIdentity) -> bool {
        identity.run_id.is_none_or(|run| {
            self.run_terminals.get(&run).is_some_and(|terminal| {
                !terminal.contradictory
                    && terminal.scope == Some(ClosureScope::from_identity(identity))
            })
        })
    }

    fn apply_run_terminal<E: LedgerEventRead>(&mut self, event: &E) {
        let EventPayload::Task(TaskPayload::Semantic(payload)) = event.payload() else {
            return;
        };
        if !matches!(payload.fact(), TaskSemanticFact::TerminalCommitted { .. }) {
            return;
        }
        let Some(run) = event.links().run_id() else {
            return;
        };
        // A second terminal cannot turn an ambiguous run into permission.
        if let Some(previous) = self.run_terminals.get_mut(run) {
            previous.contradictory = true;
            return;
        }
        let scope = self
            .owner_at(event.sequence())
            .and_then(|owner| ClosureScope::from_event(event, owner));
        self.run_terminals.insert(
            *run,
            RunTerminal {
                scope,
                contradictory: false,
            },
        );
    }

    fn close_for(&self, identity: &ArtifactRetentionIdentity) -> Option<&TerminalEvent> {
        let scope = ClosureScope::from_identity(identity);
        self.ordinary_close(&scope).or_else(|| {
            let (source, close) = self.prior_epoch_closes.get(&scope)?;
            let verified = self.objects.get(&identity.artifact.artifact_id)?.verified?;
            (verified.sequence <= close.scope_upper_sequence
                && self
                    .objects
                    .get(&identity.artifact.artifact_id)?
                    .last_reference_sequence
                    <= close.scope_upper_sequence
                && self.scope_sources.get(&scope)?.1 <= close.scope_upper_sequence)
                .then_some(source)
        })
    }

    /// Workflow #332 R-san: the close that may release the object's Explicit pins follows every
    /// pin. A capture with no run and no lease closes on its epoch's latest quiescence, which
    /// can precede its pin; such an object is kept until a later close.
    fn releasing_close(
        &self,
        object: &RetainedObject,
        identity: &ArtifactRetentionIdentity,
    ) -> Option<&TerminalEvent> {
        self.close_for(identity).filter(|close| {
            object
                .pins
                .values()
                .all(|(pin, _)| pin.sequence < close.sequence)
        })
    }

    fn ordinary_close(&self, scope: &ClosureScope) -> Option<&TerminalEvent> {
        if scope.run.is_none() {
            return match scope.lease {
                Some(lease) => self
                    .released_leases
                    .get(&(scope.owner, scope.instance, lease)),
                None => self.quiescent_instances.get(&(scope.owner, scope.instance)),
            };
        }
        self.closures.get(scope)?.close.as_ref()
    }

    pub(super) fn from_events<E: LedgerEventRead>(events: &[E]) -> GlobalLedgerResult<Self> {
        Self::from_events_checked(events, &mut |_| Ok(()))
    }

    pub(super) fn from_events_checked<E: LedgerEventRead>(
        events: &[E],
        check: &mut impl FnMut(usize) -> GlobalLedgerResult<()>,
    ) -> GlobalLedgerResult<Self> {
        Self::from_events_with_indexes_checked(events, check).map(|(retained, _)| retained)
    }

    /// Workflow #191 C: the full build, also returning the event indexes it derives.
    pub(super) fn from_events_with_indexes_checked<E: LedgerEventRead>(
        events: &[E],
        check: &mut impl FnMut(usize) -> GlobalLedgerResult<()>,
    ) -> GlobalLedgerResult<(Self, EventIndexes)> {
        let mut retained = Self::default();
        let mut indexes = EventIndexes::default();
        for (position, event) in events.iter().enumerate() {
            check(position + 1)?;
            retained.step(&mut indexes, &events[..position], event)?;
        }
        Ok((retained, indexes))
    }

    /// Workflow #191 C: one event's retention step, shared by full builds and tail extension.
    /// validate/apply (including validate_prior_epoch/apply_prior_epoch) must stay pure functions of
    /// the event sequence: no clock, store or I/O, or incremental and full results diverge.
    pub(super) fn step<E: LedgerEventRead>(
        &mut self,
        indexes: &mut EventIndexes,
        prior: &[E],
        event: &E,
    ) -> GlobalLedgerResult<()> {
        self.validate(event, prior, indexes, true, false)?;
        self.apply(event);
        indexes.insert(event, prior.len());
        Ok(())
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
            return Err(invalid(LedgerCode::ArtifactIdentityConflict.code()));
        }
        Ok(object.proof.clone().map(|mut proof| {
            proof.through_sequence = self.through_sequence;
            proof
        }))
    }

    /// `admission` is true only on the sole writer's append path; replay (every open, every read
    /// face, the writer's own startup) checks integrity only.
    pub(super) fn validate<E: LedgerEventRead>(
        &self,
        event: &E,
        events: &[E],
        indexes: &EventIndexes,
        guarded_intent: bool,
        admission: bool,
    ) -> GlobalLedgerResult<()> {
        if event.sequence() != self.through_sequence.saturating_add(1) {
            return Err(invalid(LedgerCode::ArtifactRetentionSnapshotGap.code()));
        }
        self.validate_prior_epoch(event, events, guarded_intent)?;
        // An outcome records the result of an already sealed identity, never a new use.
        let retention = event.payload().artifact_retention();
        for reference in material_references(event) {
            reference
                .validate()
                .map_err(|_| invalid(SharedCode::InvalidArtifactReference.code()))?;
            if let Some(object) = self.objects.get(&reference.artifact_id) {
                if object.reference != reference {
                    return Err(invalid(LedgerCode::ArtifactIdentityConflict.code()));
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
                && event.payload().lab_pin_release_request().is_none()
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
        if event.origin().source() != EventSource::Runtime
            || event.origin().module() != actingcommand_contract::OriginModule::ArtifactStore
            || event.origin().actor() != actingcommand_contract::EventActor::Runtime
        {
            return Err(invalid(LedgerCode::ArtifactRetentionOriginInvalid.code()));
        }
        fact.validate()
            .map_err(|_| invalid(LedgerCode::InvalidArtifactRetentionFact.code()))?;
        let identity = fact.identity();
        if !same_links(event, identity) {
            return Err(invalid(LedgerCode::ArtifactRetentionLinksConflict.code()));
        }
        let object = self
            .objects
            .get(&identity.artifact.artifact_id)
            .ok_or_else(|| invalid(LedgerCode::ArtifactRetentionSourceMissing.code()))?;
        if object.reference != identity.artifact
            || object
                .identity
                .as_ref()
                .is_some_and(|original| original != identity)
        {
            return Err(invalid(
                LedgerCode::ArtifactRetentionIdentityConflict.code(),
            ));
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
                    return Err(invalid(LedgerCode::ArtifactPinTriggerConflict.code()));
                }
                if pin.reason == ArtifactPinReason::WarningOrHigher
                    && trigger.severity() < EventSeverity::Warning
                    || pin.reason == ArtifactPinReason::Lab
                        && !indexes.lab_related(trigger, self.through_sequence)
                {
                    return Err(invalid(LedgerCode::ArtifactPinReasonConflict.code()));
                }
            }
            ArtifactRetentionFact::PinReleased(release) => {
                if object.proof.is_some() {
                    return Err(sealed());
                }
                let Some((pin, reason)) = object.pins.get(&release.pin.event_id) else {
                    return Err(invalid(LedgerCode::ArtifactPinReleaseSourceMissing.code()));
                };
                if *pin != release.pin {
                    return Err(invalid(LedgerCode::ArtifactPinNotReleasable.code()));
                }
                let release_source = source(events, &release.release)?;
                match reason {
                    ArtifactPinReason::Explicit => {
                        if admission
                            && (!same_close_scope(release_source, identity)
                                || !successful_close(release_source, identity)
                                || !self.close_owner_matches(release_source, identity))
                        {
                            return Err(invalid(LedgerCode::ArtifactPinReleaseNotClosed.code()));
                        }
                    }
                    ArtifactPinReason::Lab => {
                        let target = actingcommand_contract::LabPinReleaseTarget {
                            artifact_id: identity.artifact.artifact_id,
                            pin: release.pin,
                        };
                        if !lab_release_request_matches(release_source, &target) {
                            return Err(invalid(
                                LedgerCode::ArtifactLabPinReleaseRequestConflict.code(),
                            ));
                        }
                    }
                    _ => return Err(invalid(LedgerCode::ArtifactPinNotReleasable.code())),
                }
            }
            ArtifactRetentionFact::EvictionIntent(intent) => {
                if !guarded_intent {
                    return Err(invalid(LedgerCode::ArtifactEvictionGuardRequired.code()));
                }
                self.validate_intent(object, intent, events, indexes, admission)?;
            }
            ArtifactRetentionFact::EvictionOutcome(outcome) => {
                if !guarded_intent {
                    return Err(invalid(LedgerCode::ArtifactEvictionGuardRequired.code()));
                }
                let proof = object
                    .proof
                    .as_ref()
                    .ok_or_else(|| invalid(LedgerCode::ArtifactEvictionIntentMissing.code()))?;
                if proof.intent != outcome.intent || proof.terminal_event.is_some() {
                    return Err(invalid(LedgerCode::ArtifactEvictionOutcomeConflict.code()));
                }
            }
        }
        Ok(())
    }

    /// Workflow #332 H2f Q3: replay (`admission` false) checks only that the sealed intent is
    /// authentic and consistent with the index: its prefix, its object's identity and verified
    /// source, no earlier proof, and that every source it names resolves in the preceding
    /// prefix. Eligibility (protection, pins, Lab, close, success, summary, settlement) is
    /// decided once, by the admitting writer; a reader never re-judges it. Workflow #382 L-5b:
    /// an intent names its original success; the failed-run (K/T) evidence is retired, so a
    /// stored intent that still carries it does not decode and never reaches this check.
    fn validate_intent<E: LedgerEventRead>(
        &self,
        object: &RetainedObject,
        intent: &ArtifactEvictionIntentRecord,
        events: &[E],
        indexes: &EventIndexes,
        admission: bool,
    ) -> GlobalLedgerResult<()> {
        if intent.through_sequence != self.through_sequence
            || object.proof.is_some()
            || object.identity.as_ref() != Some(&intent.identity)
            || object.verified.as_ref() != Some(&intent.verified)
            || admission
                && (object.permanently_protected
                    || self.unlinked_warning
                    || !object.pins.is_empty())
        {
            return Err(invalid(LedgerCode::ArtifactEvictionNotEligible.code()));
        }
        let verified = source(events, &intent.verified)?;
        let Some(success) = &intent.success else {
            return Err(invalid(LedgerCode::ArtifactEvictionTerminalMissing.code()));
        };
        let success = source(events, success)?;
        let close = source(events, &intent.close)?;
        let summary = intent
            .capture_summary
            .as_ref()
            .map(|value| source(events, value))
            .transpose()?;
        let settlement = intent
            .settlement
            .as_ref()
            .map(|value| source(events, value))
            .transpose()?;
        if !admission {
            return Ok(());
        }
        if !same_scope(verified, &intent.identity)
            || !same_scope(success, &intent.identity)
            || !same_close_scope(close, &intent.identity)
            || self.lab_protected(object, verified, indexes)
            || [verified, success]
                .iter()
                .any(|event| self.owner_at(event.sequence()) != Some(intent.identity.owner_epoch))
            || !successful_close(close, &intent.identity)
            || !self.close_owner_matches(close, &intent.identity)
        {
            return Err(invalid(LedgerCode::ArtifactEvictionCloseConflict.code()));
        }
        match intent.identity.run_id {
            Some(_) => {
                if !matches!(success.payload(), EventPayload::Task(TaskPayload::Semantic(payload))
                    if matches!(payload.fact(), TaskSemanticFact::TerminalCommitted { task_result, .. } if *task_result == TaskOutcome::Success))
                {
                    return Err(invalid(
                        LedgerCode::ArtifactEvictionTaskNotSuccessful.code(),
                    ));
                }
                let summary = summary
                    .ok_or_else(|| invalid(LedgerCode::ArtifactEvictionSummaryMissing.code()))?;
                if !same_scope(summary, &intent.identity)
                    || summary.sequence() >= close.sequence()
                    || !matches!(summary.payload(), EventPayload::Capture(CapturePayload::SummaryCommitted(payload))
                        if payload.summary().frames().iter().any(|frame| frame.artifact() == &object.reference))
                {
                    return Err(invalid(LedgerCode::ArtifactEvictionSummaryConflict.code()));
                }
            }
            None if success.event_type() == EventType::InputCommitted
                && success.payload().effect_disposition() == Some(EffectDisposition::Performed) => {
            }
            None if intent.identity.lease_id.is_none()
                && success.event_type() == EventType::CaptureCompleted => {}
            None => return Err(invalid(LedgerCode::ArtifactEvictionSuccessMissing.code())),
        }
        let scheduled = intent
            .identity
            .run_id
            .is_some_and(|run| self.scheduled_runs.contains(&run));
        match (settlement, scheduled) {
            (Some(settlement), true) => {
                if !same_scope(settlement, &intent.identity)
                    || settlement.sequence() <= success.sequence()
                    || self.owner_at(settlement.sequence()) != Some(intent.identity.owner_epoch)
                    || !settlement_matches(settlement, TaskOutcome::Success)
                {
                    return Err(invalid(
                        LedgerCode::ArtifactEvictionSettlementConflict.code(),
                    ));
                }
            }
            (None, false) => {}
            _ => {
                return Err(invalid(
                    LedgerCode::ArtifactEvictionSettlementMissing.code(),
                ));
            }
        }
        Ok(())
    }

    pub(super) fn apply<E: LedgerEventRead>(&mut self, event: &E) {
        if let Some(epoch) = recorded_owner(event) {
            if let Some(previous) = self.owner_at(self.through_sequence)
                && previous != epoch
            {
                self.owner_ends.insert(previous, event.sequence() - 1);
                if self.owner_ends.contains_key(&epoch) {
                    self.reopened_owners.insert(epoch);
                }
            }
            self.owner_epochs.insert(event.sequence(), epoch);
        }
        self.apply_prior_epoch(event);
        self.apply_run_terminal(event);
        self.frame_facts.apply(event);
        if let EventPayload::Runtime(RuntimePayload::LifecycleObserved(payload)) = event.payload()
            && let RuntimeLifecyclePhase::ResourceQuiescence {
                instance_id,
                resource_count,
                quiescence: actingcommand_contract::ResourceQuiescence::Confirmed,
                owner_disposition: actingcommand_contract::OwnerResourceDisposition::ConfirmedClosed,
                ..
            } = payload.phase()
            && resource_count > 0
            && event.links().instance_id() == Some(&instance_id)
            && event.links().request_id().is_some()
            && event.links().correlation_id().is_some()
        {
            self.quiescent_instances
                .insert((payload.owner_epoch(), instance_id), terminal(event));
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
            if event.payload().artifact_retention().is_none() {
                self.scope_sources
                    .entry(scope)
                    .and_modify(|value| value.1 = event.sequence())
                    .or_insert((terminal(event), event.sequence()));
            }
            let closure = self.closures.entry(scope).or_default();
            let successful = match event.payload() {
                EventPayload::Task(TaskPayload::Semantic(payload)) => matches!(
                    payload.fact(),
                    TaskSemanticFact::TerminalCommitted {
                        task_result: TaskOutcome::Success,
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
                || scope.lease.is_none()
                    && matches!(event.payload(), EventPayload::Runtime(RuntimePayload::LifecycleObserved(payload))
                    if matches!(payload.phase(), RuntimeLifecyclePhase::ResourceQuiescence {
                        instance_id, resource_count, quiescence: actingcommand_contract::ResourceQuiescence::Confirmed,
                        owner_disposition: actingcommand_contract::OwnerResourceDisposition::ConfirmedClosed,
                        ..
                    } if instance_id == scope.instance && resource_count > 0))
            {
                closure.close = Some(terminal(event));
            }
            if matches!(
                event.payload(),
                EventPayload::Policy(PolicyPayload::ExecutionRecorded(_))
            ) {
                closure.settlement = Some(terminal(event));
            }
        }
        if let Some(run) = event.links().run_id() {
            for scope in scopes(event) {
                self.linked_runs.entry(scope).or_default().insert(*run);
                if !matches!(scope, RetentionScope::Run(_)) {
                    self.run_scopes
                        .entry(*run)
                        .or_default()
                        .entry(scope)
                        .or_insert(event.sequence());
                }
            }
            if matches!(
                event.event_type(),
                EventType::PolicyDispatchIntent | EventType::PolicyDispatchAdmitted
            ) {
                self.scheduled_runs.insert(*run);
            }
        }
        if event.payload().artifact_retention().is_none()
            && event.payload().lab_pin_release_request().is_none()
        {
            let lab = event.origin().source() == EventSource::Lab
                || event.event_type() == EventType::LabRequest;
            if lab {
                for scope in scopes(event).filter(|scope| !matches!(scope, RetentionScope::Run(_)))
                {
                    self.lab_anchors.insert(scope, event.sequence());
                }
            }
            if lab || !material_references(event).is_empty() || !referenced_frames(event).is_empty()
            {
                for scope in scopes(event) {
                    self.material_scope_activity.insert(scope, event.sequence());
                }
            }
        }
        if event.payload().artifact_retention().is_none() {
            if event.severity() >= EventSeverity::Warning
                && scopes(event).next().is_none()
                && !stale_owner_report(event)
            {
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
                    last_reference_sequence: event.sequence(),
                    verified: None,
                    summary: None,
                    identity: None,
                    pins: BTreeMap::new(),
                    released_lab_pins: BTreeMap::new(),
                    reference_scopes: BTreeSet::new(),
                    permanently_protected: protect,
                    proof: None,
                });
            object.permanently_protected |= protect;
            if event.payload().artifact_retention().is_none() {
                object.last_reference_sequence = event.sequence();
            }
            object.reference_scopes.extend(scopes(event));
            if event.event_type() == EventType::ArtifactVerified {
                object.verified.get_or_insert_with(|| terminal(event));
            }
            if event.event_type() == EventType::CaptureSummaryCommitted {
                object.summary = Some(terminal(event));
            }
        }
        for frame in referenced_frames(event) {
            if let Some(ids) = self.frames.get(&frame) {
                for id in ids {
                    if let Some(object) = self.objects.get_mut(id) {
                        object.reference_scopes.extend(scopes(event));
                        if event.payload().artifact_retention().is_none() {
                            object.last_reference_sequence = event.sequence();
                        }
                    }
                }
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
                    object.last_reference_sequence = event.sequence();
                    object
                        .pins
                        .insert(*event.event_id(), (terminal(event), pin.reason));
                }
                ArtifactRetentionFact::PinReleased(release) => {
                    if let Some((_, ArtifactPinReason::Lab)) =
                        object.pins.remove(&release.pin.event_id)
                    {
                        object.released_lab_pins.insert(
                            release.pin.event_id,
                            actingcommand_contract::LabPinReleaseResult {
                                identity: release.identity.clone(),
                                pin: release.pin,
                                release_request: release.release,
                                released: terminal(event),
                                already_released: false,
                            },
                        );
                    }
                }
                ArtifactRetentionFact::EvictionIntent(_) => {
                    self.pending_objects
                        .insert(fact.identity().artifact.artifact_id);
                    self.sealed_scopes.extend(scopes(event));
                    object.proof = Some(ArtifactEvictionProof {
                        identity: fact.identity().clone(),
                        intent: terminal(event),
                        terminal_event: None,
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
                    proof.terminal_event = Some(terminal(event));
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

    fn latest_lab_use(&self, scope: &RetentionScope) -> Option<u64> {
        match scope {
            RetentionScope::Run(run) => {
                let latest = self
                    .run_scopes
                    .get(run)?
                    .iter()
                    .filter_map(|(scope, linked)| {
                        self.latest_lab_use(scope)
                            .map(|sequence| sequence.max(*linked))
                    })
                    .max()?;
                Some(
                    latest.max(
                        self.material_scope_activity
                            .get(scope)
                            .copied()
                            .unwrap_or(0),
                    ),
                )
            }
            _ => self.lab_anchors.get(scope).map(|anchor| {
                (*anchor).max(
                    self.material_scope_activity
                        .get(scope)
                        .copied()
                        .unwrap_or(0),
                )
            }),
        }
    }

    fn lab_protected<E: LedgerEventRead>(
        &self,
        object: &RetainedObject,
        verified: &E,
        indexes: &EventIndexes,
    ) -> bool {
        let latest = object
            .reference_scopes
            .iter()
            .filter_map(|scope| self.latest_lab_use(scope))
            .max();
        if !indexes.lab_related(verified, self.through_sequence) && latest.is_none() {
            return false;
        }
        let covered = object
            .released_lab_pins
            .values()
            .map(|release| release.release_request.sequence)
            .max();
        match (latest, covered) {
            (Some(latest), Some(covered)) => {
                latest > covered
                    || object
                        .pins
                        .values()
                        .any(|(_, reason)| *reason == ArtifactPinReason::Lab)
            }
            _ => true,
        }
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
    if let EventPayload::Fact(FactPayload::Published(payload)) = event.payload() {
        for record in payload.records() {
            if let FactContent::Artifact { artifact } = &record.content {
                ids.insert(artifact.artifact_id);
            }
        }
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
        .ok_or_else(|| invalid(LedgerCode::ArtifactRetentionProofSourceMissing.code()))
}

fn terminal<E: LedgerEventRead>(event: &E) -> TerminalEvent {
    TerminalEvent {
        event_id: *event.event_id(),
        sequence: event.sequence(),
    }
}

fn lab_release_request_matches<E: LedgerEventRead>(
    event: &E,
    target: &actingcommand_contract::LabPinReleaseTarget,
) -> bool {
    event.event_type() == EventType::LabRequest
        && event.origin().source() == EventSource::Lab
        && event.origin().actor() == actingcommand_contract::EventActor::Lab
        && event.origin().module() == actingcommand_contract::OriginModule::Actinglab
        && event.links().request_id().is_some()
        && event.links().correlation_id().is_some()
        && event.sequence() > target.pin.sequence
        && event.payload().lab_pin_release_request() == Some(target)
}

fn settlement_matches<E: LedgerEventRead>(event: &E, outcome: TaskOutcome) -> bool {
    let EventPayload::Policy(PolicyPayload::ExecutionRecorded(payload)) = event.payload() else {
        return false;
    };
    match (outcome, payload.policy_result()) {
        (TaskOutcome::Success, PolicyExecutionOutcome::Succeeded { .. }) => true,
        (
            TaskOutcome::Failure | TaskOutcome::Cancelled,
            PolicyExecutionOutcome::Failed { failure },
        ) => !failure.reported_success,
        _ => false,
    }
}

fn retention_now() -> GlobalLedgerResult<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|time| u64::try_from(time.as_millis()).ok())
        .filter(|time| *time > 0)
        .ok_or_else(|| invalid(LedgerCode::ArtifactRetentionClockFailed.code()))
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
    if let Some(actingcommand_contract::PriorEpochCloseFact::ScopeClosed(close)) =
        prior_epoch::fact(event)
    {
        return ClosureScope::from_subject(close.subject) == ClosureScope::from_identity(identity);
    }
    if identity.run_id.is_none() {
        return event.links().instance_id() == Some(&identity.instance_id)
            && match identity.lease_id {
                Some(lease) => {
                    event.links().lease_id() == Some(&lease) && event.links().run_id().is_none()
                }
                None => {
                    event.links().request_id().is_some()
                        && event.links().correlation_id().is_some()
                        && successful_close(event, identity)
                }
            };
    }
    same_scope(event, identity)
}

fn successful_close<E: LedgerEventRead>(event: &E, identity: &ArtifactRetentionIdentity) -> bool {
    if let Some(actingcommand_contract::PriorEpochCloseFact::ScopeClosed(close)) =
        prior_epoch::fact(event)
    {
        return ClosureScope::from_subject(close.subject) == ClosureScope::from_identity(identity);
    }
    match (identity.lease_id, event.payload()) {
        (Some(_), EventPayload::Lease(LeasePayload::Released(payload))) => {
            payload.effect_disposition() == EffectDisposition::Performed
        }
        (None, EventPayload::Runtime(RuntimePayload::LifecycleObserved(payload))) => {
            payload.owner_epoch() == identity.owner_epoch
                && matches!(payload.phase(), RuntimeLifecyclePhase::ResourceQuiescence {
                instance_id, resource_count, quiescence: actingcommand_contract::ResourceQuiescence::Confirmed,
                owner_disposition: actingcommand_contract::OwnerResourceDisposition::ConfirmedClosed,
                ..
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

/// Workflow #332 H2f M3: the writer's own stale-owner takeover report names no material or
/// scope and lost no bytes; it is not an unlinked warning.
fn stale_owner_report<E: LedgerEventRead>(event: &E) -> bool {
    matches!(event.payload(), EventPayload::Ledger(actingcommand_contract::LedgerPayload::Recovered(recovery))
        if recovery.reason() == actingcommand_contract::RecoveryReason::StaleOwner
            && recovery.affected_bytes() == 0)
}

fn recorded_owner<E: LedgerEventRead>(event: &E) -> Option<OwnerEpoch> {
    if let Some(capacity) = event
        .payload()
        .performance_summary()
        .and_then(|summary| summary.capacity())
    {
        return Some(capacity.owner_epoch);
    }
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

fn invalid(code: Code) -> GlobalLedgerError {
    GlobalLedgerError::fatal(code, LedgerLocation::ArtifactRetention.location())
}

impl<B: super::storage::DurableStorage> super::storage::EventStore<B> {
    pub(super) fn release_lab_pin(
        &mut self,
        target: actingcommand_contract::LabPinReleaseTarget,
        request: TerminalEvent,
    ) -> GlobalLedgerResult<(
        actingcommand_contract::LabPinReleaseResult,
        Vec<PersistedEvent>,
    )> {
        let denied =
            |code| GlobalLedgerError::request(code, ContractLocation::ReleaseLabPin.location());
        let request_event = source(&self.events, &request)
            .map_err(|_| denied(LedgerCode::LabPinReleaseRequestMissing.code()))?;
        if !lab_release_request_matches(request_event, &target) {
            return Err(denied(LedgerCode::LabPinReleaseRequestConflict.code()));
        }
        let object = self
            .retention
            .objects
            .get(&target.artifact_id)
            .ok_or_else(|| denied(LedgerCode::LabPinReleaseArtifactUnknown.code()))?;
        if object.proof.is_some() {
            return Err(denied(LedgerCode::LabPinReleaseArtifactSealed.code()));
        }
        if let Some(released) = object.released_lab_pins.get(&target.pin.event_id) {
            if released.pin != target.pin {
                return Err(denied(LedgerCode::LabPinReleasePinConflict.code()));
            }
            let mut result = released.clone();
            result.already_released = true;
            return Ok((result, Vec::new()));
        }
        let Some((pin, reason)) = object.pins.get(&target.pin.event_id) else {
            return Err(denied(LedgerCode::LabPinReleasePinUnknown.code()));
        };
        if *pin != target.pin || *reason != ArtifactPinReason::Lab {
            return Err(denied(LedgerCode::LabPinReleasePinNotLab.code()));
        }
        let identity = object
            .identity
            .clone()
            .ok_or_else(|| denied(LedgerCode::LabPinReleaseIdentityMissing.code()))?;
        let fact =
            ArtifactRetentionFact::PinReleased(actingcommand_contract::ArtifactPinReleaseRecord {
                identity: identity.clone(),
                pin: *pin,
                release: request,
            });
        let persisted = self.append_retention_fact(fact, &target.pin, false)?;
        Ok((
            actingcommand_contract::LabPinReleaseResult {
                identity,
                pin: target.pin,
                release_request: request,
                released: terminal(&persisted),
                already_released: false,
            },
            vec![persisted],
        ))
    }

    pub(super) fn retention_candidates(
        &self,
        after: Option<ArtifactId>,
    ) -> GlobalLedgerResult<ArtifactRetentionCandidates> {
        Ok(self.retention.candidates(after, self.recovering_retention))
    }

    /// The material guard arrived before this command; the writer performs no material I/O.
    pub(super) fn admit_artifact_eviction(
        &mut self,
        guard: ArtifactDeleteGuard,
    ) -> GlobalLedgerResult<(ArtifactEvictionAdmission, Vec<PersistedEvent>)> {
        if guard.root() != self.backend.material_root() {
            return Err(invalid(LedgerCode::ArtifactEvictionRootConflict.code()));
        }
        let reference = guard.reference().clone();
        if let Some(proof) = self.retention.proof(&reference)? {
            if proof.terminal_event.is_some() {
                return Ok((ArtifactEvictionAdmission::Deferred, Vec::new()));
            }
            if !self.recovering_retention {
                return Err(GlobalLedgerError::request(
                    LedgerCode::ArtifactEvictionIntentPending.code(),
                    LedgerLocation::AdmitArtifactEviction.location(),
                ));
            }
            // Recovery re-checks the material under this exclusive, hash-verified guard: absent
            // material records RecoveryAbsent; present material completes the sealed unlink once.
            let original = source(&self.events, &proof.intent)?;
            let Some(ArtifactRetentionFact::EvictionIntent(intent)) =
                original.payload().artifact_retention()
            else {
                return Err(invalid(LedgerCode::ArtifactEvictionIntentInvalid.code()));
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
        let verified = *verified;
        let releases = self
            .retention
            .releasing_close(object, &identity)
            .map(|close| {
                object
                    .pins
                    .values()
                    .filter(|(_, reason)| *reason == ArtifactPinReason::Explicit)
                    .take(RETENTION_ROUND_OBJECTS)
                    .map(
                        |(pin, _)| actingcommand_contract::ArtifactPinReleaseRecord {
                            identity: identity.clone(),
                            pin: *pin,
                            release: *close,
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
            return Err(invalid(LedgerCode::ArtifactStoreVerificationFailed.code()));
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
        outcome: Option<actingcommand_contract::outcome::Outcome>,
    ) -> GlobalLedgerResult<PersistedEvent> {
        if permit.guard.root() != self.backend.material_root()
            || permit.guard.reference() != &permit.intent.identity.artifact
            || disposition == ArtifactEvictionDisposition::RecoveryAbsent
                && (!permit.recovery || permit.guard.material_present())
        {
            return Err(invalid(
                LedgerCode::ArtifactEvictionOutcomeGuardConflict.code(),
            ));
        }
        let fact = ArtifactRetentionFact::EvictionOutcome(
            actingcommand_contract::ArtifactEvictionOutcomeRecord {
                identity: permit.intent.identity,
                intent: permit.source,
                disposition,
                outcome,
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
        // The stored severity is the record's category: a failed eviction's I/O outcome, else
        // the event type's (ledger revision 3, Workflow #378 A3).
        let severity = EventSeverity::Info;
        let ids = IdentifierIssuer::new().map_err(|error| invalid(error.code()))?;
        let timestamp = retention_now()?;
        let draft = EventDraft::new(
            ids.mint_event_id().map_err(|error| invalid(error.code()))?,
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
        // The refused field or rule stays in the developer detail; the operation is the
        // registered retention location (Workflow #378 A2-P: it was the field's name).
        .map_err(|error| {
            GlobalLedgerError::fatal(error.code(), LedgerLocation::ArtifactRetention.location())
                .with_detail(error.to_string())
        })?;
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
        Option<actingcommand_contract::outcome::Outcome>,
    ) {
        if self.recovery && !self.guard.material_present() {
            return (ArtifactEvictionDisposition::RecoveryAbsent, None);
        }
        match self.guard.remove_after_durable_intent(&self.intent) {
            Ok(()) => (ArtifactEvictionDisposition::Deleted, None),
            Err(error) => (
                ArtifactEvictionDisposition::Failed,
                Some(actingcommand_contract::artifact_eviction_io_outcome(&error)),
            ),
        }
    }
}

impl super::GlobalLedger {
    pub fn release_lab_pin(
        &self,
        target: actingcommand_contract::LabPinReleaseTarget,
        request: TerminalEvent,
    ) -> GlobalLedgerResult<actingcommand_contract::LabPinReleaseResult> {
        target.validate().map_err(|_| {
            GlobalLedgerError::request(
                SharedCode::InvalidLabPinReleaseTarget.code(),
                ContractLocation::ReleaseLabPin.location(),
            )
        })?;
        let (response, receiver) = std::sync::mpsc::sync_channel(1);
        let sender = self
            .sender
            .as_ref()
            .ok_or_else(|| invalid(LedgerCode::WriterUnavailable.code()))?;
        super::send_command(
            sender,
            super::WriterCommand::ReleaseLabPin {
                target,
                request,
                response,
            },
            ContractLocation::ReleaseLabPin.location(),
        )?;
        super::receive_response(
            receiver,
            ContractLocation::ReleaseLabPin.location(),
            self.reply_timeout,
        )?
    }

    pub fn retention_candidates(
        &self,
        after: Option<ArtifactId>,
    ) -> GlobalLedgerResult<ArtifactRetentionCandidates> {
        let (response, receiver) = std::sync::mpsc::sync_channel(1);
        let sender = self
            .sender
            .as_ref()
            .ok_or_else(|| invalid(LedgerCode::WriterUnavailable.code()))?;
        super::send_command(
            sender,
            super::WriterCommand::RetentionCandidates { after, response },
            LedgerLocation::RetentionCandidates.location(),
        )?;
        super::receive_response(
            receiver,
            LedgerLocation::RetentionCandidates.location(),
            self.reply_timeout,
        )?
    }

    pub fn admit_artifact_eviction(
        &self,
        guard: ArtifactDeleteGuard,
    ) -> GlobalLedgerResult<ArtifactEvictionAdmission> {
        let (response, receiver) = std::sync::mpsc::sync_channel(1);
        let sender = self
            .sender
            .as_ref()
            .ok_or_else(|| invalid(LedgerCode::WriterUnavailable.code()))?;
        super::send_command(
            sender,
            super::WriterCommand::AdmitArtifactEviction {
                guard: Box::new(guard),
                response,
            },
            LedgerLocation::AdmitArtifactEviction.location(),
        )?;
        super::receive_response(
            receiver,
            LedgerLocation::AdmitArtifactEviction.location(),
            self.reply_timeout,
        )?
    }

    pub fn finish_artifact_eviction(
        &self,
        mut permit: Box<ArtifactEvictionPermit>,
    ) -> GlobalLedgerResult<PersistedEvent> {
        self.check_writer_health()?;
        let sender = self
            .sender
            .as_ref()
            .ok_or_else(|| invalid(LedgerCode::WriterUnavailable.code()))?;
        let (disposition, outcome) = permit.perform();
        let (response, receiver) = std::sync::mpsc::sync_channel(1);
        super::send_command(
            sender,
            super::WriterCommand::FinishArtifactEviction {
                permit,
                disposition,
                outcome,
                response,
            },
            LedgerLocation::FinishArtifactEviction.location(),
        )?;
        super::receive_response(
            receiver,
            LedgerLocation::FinishArtifactEviction.location(),
            self.reply_timeout,
        )?
    }
}

fn sealed() -> GlobalLedgerError {
    GlobalLedgerError::request(
        LedgerCode::ArtifactMaterialAdmissionClosed.code(),
        LedgerLocation::AppendEvent.location(),
    )
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
    restore_records_with(&retention, records, verifier, check)
}

/// Workflow #191 C: the restoring half of `restore_records`, against a retention index
/// already derived from the same authenticated records. Workflow #375 H1: each distinct
/// reference without an eviction proof is verified once, in first-reference order, before
/// the records are restored in sequence order.
pub(super) fn restore_records_with<F>(
    retention: &RetentionIndex,
    records: Vec<StoredEventRecord>,
    verifier: &mut Option<F>,
    mut check: impl FnMut(usize) -> GlobalLedgerResult<()>,
) -> GlobalLedgerResult<Vec<PersistedEvent>>
where
    F: FnMut(&ProjectedArtifactReference) -> Option<VerifiedArtifactReference>,
{
    let verdicts = match verifier.as_mut() {
        Some(verify) => {
            Some(PendingMaterial::collect(retention, &records).verify_each(verify, &mut check)?)
        }
        None => None,
    };
    restore_verified(retention, records, verdicts.as_ref(), check)
}

/// Workflow #375 R5a: the restoring half of a formal opening (the writer open and every
/// `ledger-maintenance` read), which never opens, hashes or stats artifact material. An
/// eviction proof gives its availability; any other reference stays `Unrecorded`. Only
/// `timing.restore_ms` is measured: the material fields stay 0. `timing` keeps the phase
/// also when an error is returned.
pub(super) fn restore_records_unread(
    retention: &RetentionIndex,
    records: Vec<StoredEventRecord>,
    mut check: impl FnMut(usize) -> GlobalLedgerResult<()>,
    timing: &mut LedgerOpenTiming,
) -> GlobalLedgerResult<Vec<PersistedEvent>> {
    let started = std::time::Instant::now();
    let restored = (|| -> GlobalLedgerResult<Vec<PersistedEvent>> {
        let mut events = Vec::with_capacity(records.len());
        for record in records {
            check(events.len() + 1)?;
            let event = retention.restore_record_unread(record)?;
            check(events.len() + 1)?;
            events.push(event);
        }
        Ok(events)
    })();
    timing.restore_ms = elapsed_ms(started);
    restored
}

/// Restores in sequence order with the verdicts of the pending material; `None` means no
/// verifier was given, so an artifact without a proof fails as before.
fn restore_verified(
    retention: &RetentionIndex,
    records: Vec<StoredEventRecord>,
    verdicts: Option<&material::MaterialVerdicts>,
    mut check: impl FnMut(usize) -> GlobalLedgerResult<()>,
) -> GlobalLedgerResult<Vec<PersistedEvent>> {
    let mut lookup = verdicts
        .map(|verdicts| move |reference: &ProjectedArtifactReference| verdicts.get(reference));
    let mut events = Vec::with_capacity(records.len());
    for record in records {
        check(events.len() + 1)?;
        let event = retention.restore_record(record, &mut lookup)?;
        check(events.len() + 1)?;
        events.push(event);
    }
    Ok(events)
}

impl RetentionIndex {
    /// The availability an authenticated eviction proof gives a reference, or `None` when it
    /// has none. Workflow #375 R5a: a `Failed` outcome restores as `FailedEviction`; it no
    /// longer stops an opening. An identity conflict stays fatal.
    fn proven_availability(
        &self,
        reference: &ProjectedArtifactReference,
    ) -> Result<Option<ArtifactAvailability>, FactValidationError> {
        // `RetentionIndex::proof` fails only with `artifact_identity_conflict`.
        let proof = self
            .proof(reference)
            .map_err(|_| FactValidationError::new(LedgerCode::ArtifactIdentityConflict.code()))?;
        Ok(proof.map(proof_availability))
    }

    /// Restores a stored record whose unproven material must verify: a missing verifier or a
    /// verifier `None` fails the opening. Only the candidate and Segment openings use it.
    pub(super) fn restore_record<F>(
        &self,
        record: StoredEventRecord,
        verifier: &mut Option<F>,
    ) -> GlobalLedgerResult<PersistedEvent>
    where
        F: FnMut(&ProjectedArtifactReference) -> Option<VerifiedArtifactReference>,
    {
        let mut event = record
            .into_event_with_artifact_availability(&mut |reference| {
                if let Some(availability) = self.proven_availability(reference)? {
                    return Ok(Some(availability));
                }
                let verify = verifier.as_mut().ok_or(FactValidationError::new(
                    LedgerCode::ArtifactStoreVerificationUnavailable.code(),
                ))?;
                verify(reference)
                    .map(|verified| Some(ArtifactAvailability::Available(verified)))
                    .ok_or(FactValidationError::new(
                        LedgerCode::ArtifactStoreVerificationFailed.code(),
                    ))
            })
            .map_err(|error| {
                GlobalLedgerError::fatal(
                    error.code(),
                    LedgerLocation::ValidatePersistedEvent.location(),
                )
            })?;
        self.annotate_event(&mut event);
        Ok(event)
    }

    /// Workflow #375 R5a: restores a stored record without reading its material. A reference
    /// with an eviction proof takes the proof's availability; any other stays `Unrecorded`,
    /// so a missing or damaged artifact file never fails the record.
    pub(super) fn restore_record_unread(
        &self,
        record: StoredEventRecord,
    ) -> GlobalLedgerResult<PersistedEvent> {
        let mut event = record
            .into_event_with_artifact_availability(&mut |reference| {
                self.proven_availability(reference)
            })
            .map_err(|error| {
                GlobalLedgerError::fatal(
                    error.code(),
                    LedgerLocation::ValidatePersistedEvent.location(),
                )
            })?;
        self.annotate_event(&mut event);
        Ok(event)
    }

    /// Restores authenticated metadata with per-artifact material state. Eviction proofs
    /// are applied as in `restore_record`, and an identity conflict stays fatal. Unlike
    /// `restore_record`, an artifact without a proof never fails the event: with no
    /// verifier its material is not read, and a verifier `None` leaves it Unrecorded. The
    /// caller must record and report every such `None`.
    pub(super) fn restore_metadata<F>(
        &self,
        event: crate::fact::LedgerEventMetadata,
        verifier: &mut Option<F>,
    ) -> GlobalLedgerResult<PersistedEvent>
    where
        F: FnMut(&ProjectedArtifactReference) -> Option<VerifiedArtifactReference>,
    {
        let mut event = event
            .into_event_with_artifact_availability(&mut |reference| {
                if let Some(availability) = self.proven_availability(reference)? {
                    return Ok(Some(availability));
                }
                Ok(verifier
                    .as_mut()
                    .and_then(|verify| verify(reference))
                    .map(ArtifactAvailability::Available))
            })
            .map_err(|error| {
                GlobalLedgerError::fatal(
                    error.code(),
                    LedgerLocation::ValidatePersistedEvent.location(),
                )
            })?;
        self.annotate_event(&mut event);
        Ok(event)
    }
}

/// The availability an eviction proof gives a reference.
fn proof_availability(proof: ArtifactEvictionProof) -> ArtifactAvailability {
    match proof.disposition {
        Some(
            ArtifactEvictionDisposition::Deleted | ArtifactEvictionDisposition::RecoveryAbsent,
        ) => ArtifactAvailability::Evicted(Box::new(proof)),
        None => ArtifactAvailability::PendingEviction(Box::new(proof)),
        Some(ArtifactEvictionDisposition::Failed) => {
            ArtifactAvailability::FailedEviction(Box::new(proof))
        }
    }
}

/// Workflow #411 A4: the eviction proofs of a range read's matching events, built from the
/// eviction intent and outcome rows that the read authenticated each on its own, in ledger
/// order, not from a retention replay of the whole ledger. A proof carries the head of the read
/// transaction, as a replay's proof carries the replay's head. Every reference of an artifact
/// precedes its intent (a replay refuses a later one), so the intents and outcomes of the
/// matching events' artifacts all follow the first matching event.
pub(super) struct RangeProofs {
    head: u64,
    /// The artifacts the matching events reference, with their reference.
    wanted: BTreeMap<ArtifactId, ProjectedArtifactReference>,
    proofs: BTreeMap<ArtifactId, ArtifactEvictionProof>,
}

impl RangeProofs {
    pub(super) fn new(head: u64) -> Self {
        Self {
            head,
            wanted: BTreeMap::new(),
            proofs: BTreeMap::new(),
        }
    }

    /// Whether a matching event references an artifact.
    pub(super) fn wants_any(&self) -> bool {
        !self.wanted.is_empty()
    }

    /// Records the artifacts a matching event references. A reference that differs from an
    /// earlier one of the same artifact is a replay's identity conflict, and a reference after
    /// the artifact's eviction intent a replay's closed admission.
    pub(super) fn want<E: LedgerEventRead>(&mut self, event: &E) -> GlobalLedgerResult<()> {
        for reference in material_references(event) {
            if self.proofs.contains_key(&reference.artifact_id) {
                return Err(sealed());
            }
            match self.wanted.get(&reference.artifact_id) {
                Some(known) if *known != reference => {
                    return Err(invalid(LedgerCode::ArtifactIdentityConflict.code()));
                }
                Some(_) => {}
                None => {
                    self.wanted.insert(reference.artifact_id, reference);
                }
            }
        }
        Ok(())
    }

    /// Applies one authenticated row in ledger order. An eviction intent or outcome must pass
    /// the checks a replay makes on the row alone (its origin, the fact, its links, an intent's
    /// prefix and success); one of a wanted artifact must name that artifact's reference, an
    /// intent must be its first and an outcome must answer it once, with a replay's codes.
    /// Every other row is left as it is.
    pub(super) fn apply<E: LedgerEventRead>(&mut self, event: &E) -> GlobalLedgerResult<()> {
        let Some(fact) = event.payload().artifact_retention() else {
            return Ok(());
        };
        if !matches!(
            fact,
            ArtifactRetentionFact::EvictionIntent(_) | ArtifactRetentionFact::EvictionOutcome(_)
        ) {
            return Ok(());
        }
        if event.origin().source() != EventSource::Runtime
            || event.origin().module() != actingcommand_contract::OriginModule::ArtifactStore
            || event.origin().actor() != actingcommand_contract::EventActor::Runtime
        {
            return Err(invalid(LedgerCode::ArtifactRetentionOriginInvalid.code()));
        }
        fact.validate()
            .map_err(|_| invalid(LedgerCode::InvalidArtifactRetentionFact.code()))?;
        let identity = fact.identity();
        if !same_links(event, identity) {
            return Err(invalid(LedgerCode::ArtifactRetentionLinksConflict.code()));
        }
        if let ArtifactRetentionFact::EvictionIntent(intent) = fact {
            if intent.through_sequence.checked_add(1) != Some(event.sequence()) {
                return Err(invalid(LedgerCode::ArtifactEvictionNotEligible.code()));
            }
            if intent.success.is_none() {
                return Err(invalid(LedgerCode::ArtifactEvictionTerminalMissing.code()));
            }
        }
        let artifact_id = identity.artifact.artifact_id;
        let Some(reference) = self.wanted.get(&artifact_id) else {
            return Ok(());
        };
        if *reference != identity.artifact {
            return Err(invalid(
                LedgerCode::ArtifactRetentionIdentityConflict.code(),
            ));
        }
        match fact {
            ArtifactRetentionFact::EvictionIntent(_) => {
                if self.proofs.contains_key(&artifact_id) {
                    return Err(invalid(LedgerCode::ArtifactEvictionNotEligible.code()));
                }
                self.proofs.insert(
                    artifact_id,
                    ArtifactEvictionProof {
                        identity: identity.clone(),
                        intent: terminal(event),
                        terminal_event: None,
                        disposition: None,
                        through_sequence: self.head,
                    },
                );
            }
            ArtifactRetentionFact::EvictionOutcome(outcome) => {
                let proof = self
                    .proofs
                    .get_mut(&artifact_id)
                    .ok_or_else(|| invalid(LedgerCode::ArtifactEvictionIntentMissing.code()))?;
                if proof.intent != outcome.intent || proof.terminal_event.is_some() {
                    return Err(invalid(LedgerCode::ArtifactEvictionOutcomeConflict.code()));
                }
                proof.terminal_event = Some(terminal(event));
                proof.disposition = Some(outcome.disposition);
            }
            ArtifactRetentionFact::PinRecorded(_) | ArtifactRetentionFact::PinReleased(_) => {}
        }
        Ok(())
    }

    /// One event's proofs, as a replay annotates them: one per artifact it references that has
    /// a proof, in reference order.
    pub(super) fn event_proofs<E: LedgerEventRead>(&self, event: &E) -> Vec<ArtifactEvictionProof> {
        let mut seen = BTreeSet::new();
        material_references(event)
            .into_iter()
            .filter(|reference| seen.insert(reference.artifact_id))
            .filter_map(|reference| self.proofs.get(&reference.artifact_id).cloned())
            .collect()
    }

    /// The availability a proof gives a reference, or `None` when it has none.
    pub(super) fn availability(
        &self,
        reference: &ProjectedArtifactReference,
    ) -> Result<Option<ArtifactAvailability>, FactValidationError> {
        Ok(self
            .proofs
            .get(&reference.artifact_id)
            .cloned()
            .map(proof_availability))
    }
}
