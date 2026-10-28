// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::owner_journal::{OwnerEpochBlock, RuntimeOwnerJournal};
use actingcommand_contract::{
    OwnerResourceDisposition, PriorEpochCloseBasis, PriorEpochCloseFact, PriorEpochOwnerImport,
    PriorEpochScope, PriorEpochScopeClose, UnprovenReason,
};
use std::{sync::Arc, time::Instant};

/// One page of prior-epoch scopes and the cursor of the page after it.
type PriorEpochPage = (
    Vec<(ClosureScope, (TerminalEvent, u64))>,
    Option<PriorEpochScope>,
);

impl ClosureScope {
    pub(super) fn from_subject(value: PriorEpochScope) -> Self {
        Self {
            owner: value.owner,
            instance: value.instance,
            run: value.run,
            lease: value.lease,
            request: value.request,
            correlation: value.correlation,
        }
    }
    fn subject(self) -> PriorEpochScope {
        PriorEpochScope {
            owner: self.owner,
            instance: self.instance,
            run: self.run,
            lease: self.lease,
            request: self.request,
            correlation: self.correlation,
        }
    }
}

pub(super) fn fact<E: LedgerEventRead>(event: &E) -> Option<&PriorEpochCloseFact> {
    match event.payload() {
        EventPayload::Runtime(RuntimePayload::LifecycleObserved(payload)) => {
            payload.prior_epoch_close()
        }
        _ => None,
    }
}

impl RetentionIndex {
    fn prior_epoch_upper(&self, owner: OwnerEpoch) -> Option<u64> {
        (!self.reopened_owners.contains(&owner))
            .then(|| self.owner_ends.get(&owner).copied())
            .flatten()
    }

    pub(super) fn validate_prior_epoch<E: LedgerEventRead, H: RetentionHistory<Event = E>>(
        &self,
        event: &E,
        history: &H,
        guarded: bool,
    ) -> GlobalLedgerResult<()> {
        let Some(observation) = fact(event) else {
            return Ok(());
        };
        let writer = recorded_owner(event)
            .ok_or_else(|| invalid(LedgerCode::PriorEpochWriterMissing.code()))?;
        if !guarded
            || self.owner_at(self.through_sequence) != Some(writer)
            || event.origin().source() != EventSource::Runtime
            || event.origin().module() != actingcommand_contract::OriginModule::GlobalLedger
            || event.origin().actor() != actingcommand_contract::EventActor::Runtime
            || event.links().instance_id().is_some()
            || event.links().run_id().is_some()
            || event.links().lease_id().is_some()
            || event.links().frame_id().is_some()
            || event.links().request_id().is_none()
            || event.links().correlation_id().is_none()
            || event.links().action_id().is_none()
        {
            return Err(invalid(LedgerCode::PriorEpochCloseAdmissionConflict.code()));
        }
        observation
            .validate(writer)
            .map_err(|_| invalid(LedgerCode::PriorEpochCloseInvalid.code()))?;
        match observation {
            PriorEpochCloseFact::OwnerImported(record) => {
                if record.through_sequence != self.through_sequence
                    || self.prior_epoch_upper(record.evidence.subject)
                        != Some(record.scope_upper_sequence)
                    || self.owner_imports.contains_key(&record.evidence.subject)
                {
                    return Err(invalid(LedgerCode::PriorEpochImportPrefixConflict.code()));
                }
            }
            PriorEpochCloseFact::ScopeClosed(record) => {
                let scope = ClosureScope::from_subject(record.subject);
                let Some((proof_source, proof)) = self.owner_imports.get(&scope.owner) else {
                    return Err(invalid(LedgerCode::PriorEpochImportMissing.code()));
                };
                if *proof_source != record.proof
                    || proof.basis != record.basis
                    || proof.through_sequence != record.through_sequence
                    || proof.scope_upper_sequence != record.scope_upper_sequence
                    || self.prior_epoch_upper(scope.owner) != Some(record.scope_upper_sequence)
                    || self.scope_sources.get(&scope).is_none_or(|(first, last)| {
                        *first != record.scope_source || *last > record.scope_upper_sequence
                    })
                    || self.ordinary_close(&scope).is_some()
                    || self.prior_epoch_closes.contains_key(&scope)
                {
                    return Err(invalid(LedgerCode::PriorEpochScopePrefixConflict.code()));
                }
                let original = source(history, &record.scope_source)?;
                if self.owner_at(original.sequence()) != Some(scope.owner)
                    || ClosureScope::from_event(&original, scope.owner) != Some(scope)
                {
                    return Err(invalid(LedgerCode::PriorEpochScopeSourceConflict.code()));
                }
                let imported = source(history, &record.proof)?;
                if !matches!(fact(&imported), Some(PriorEpochCloseFact::OwnerImported(value)) if value == proof)
                {
                    return Err(invalid(LedgerCode::PriorEpochImportSourceConflict.code()));
                }
            }
        }
        Ok(())
    }

    pub(super) fn apply_prior_epoch<E: LedgerEventRead>(&mut self, event: &E) {
        let subject = match fact(event) {
            Some(PriorEpochCloseFact::OwnerImported(record)) => {
                self.owner_imports
                    .insert(record.evidence.subject, (terminal(event), record.clone()));
                record.evidence.subject
            }
            Some(PriorEpochCloseFact::ScopeClosed(record)) => {
                self.prior_epoch_closes.insert(
                    ClosureScope::from_subject(record.subject),
                    (terminal(event), record.clone()),
                );
                record.subject.owner
            }
            None => return,
        };
        if let Some(writer) = recorded_owner(event) {
            self.prior_epoch_writers.insert(subject, writer);
        }
    }

    /// LN-7 (#422, LEAN-L §3.5): the owners whose scopes the start of `writer` closes: the owner
    /// of the epoch that ended right before the writer's, and each owner whose import or a scope
    /// close of it that epoch wrote, whose closes this start resumes (that start may have stopped
    /// before it closed every scope). An older epoch was closed by the start that followed it.
    pub(super) fn prior_epoch_owners(&self, writer: OwnerEpoch) -> BTreeSet<OwnerEpoch> {
        let Some(previous) = self
            .owner_epochs
            .values()
            .rev()
            .copied()
            .find(|epoch| *epoch != writer)
        else {
            return BTreeSet::new();
        };
        let mut owners = self
            .prior_epoch_writers
            .iter()
            .filter(|(subject, written_by)| **written_by == previous && **subject != writer)
            .map(|(subject, _)| *subject)
            .collect::<BTreeSet<_>>();
        owners.insert(previous);
        owners
    }

    /// LN-7: the next scopes of `owners` after `after`, at most `RETENTION_ROUND_OBJECTS` of
    /// them, with the cursor of the page after them. The scopes are in key order, so the pages
    /// of one start together pass over the scope keys once.
    pub(super) fn prior_epoch_page(
        &self,
        owners: &BTreeSet<OwnerEpoch>,
        after: Option<PriorEpochScope>,
    ) -> PriorEpochPage {
        use std::ops::Bound::{Excluded, Unbounded};
        let page = self
            .scope_sources
            .range((
                after
                    .map(ClosureScope::from_subject)
                    .map_or(Unbounded, Excluded),
                Unbounded,
            ))
            .filter(|(scope, _)| owners.contains(&scope.owner))
            .take(RETENTION_ROUND_OBJECTS)
            .map(|(scope, value)| (*scope, *value))
            .collect::<Vec<_>>();
        let next = (page.len() == RETENTION_ROUND_OBJECTS)
            .then(|| page.last().expect("full page").0.subject());
        (page, next)
    }

    pub(super) fn close_owner_matches<E: LedgerEventRead>(
        &self,
        event: &E,
        identity: &ArtifactRetentionIdentity,
    ) -> bool {
        if let Some(PriorEpochCloseFact::ScopeClosed(record)) = fact(event) {
            let scope = ClosureScope::from_identity(identity);
            return self
                .prior_epoch_closes
                .get(&scope)
                .is_some_and(|(source, closed)| *source == terminal(event) && closed == record)
                && recorded_owner(event).is_some_and(|writer| {
                    writer != identity.owner_epoch
                        && self.owner_at(event.sequence()) == Some(writer)
                })
                && self
                    .scope_sources
                    .get(&scope)
                    .is_some_and(|(_, last)| *last <= record.scope_upper_sequence)
                && self
                    .objects
                    .get(&identity.artifact.artifact_id)
                    .is_some_and(|object| {
                        object.verified.is_some_and(|verified| {
                            verified.sequence <= record.scope_upper_sequence
                        }) && object.last_reference_sequence <= record.scope_upper_sequence
                    });
        }
        self.owner_at(event.sequence()) == Some(identity.owner_epoch)
    }
}

fn deadline_check(deadline: Instant) -> GlobalLedgerResult<()> {
    if Instant::now() >= deadline {
        return Err(invalid(LedgerCode::PriorEpochCloseDeadline.code()));
    }
    Ok(())
}

/// Why an owner without a native proof is closed anyway: every unproven prior epoch closes
/// (Workflow #394 EPOCH-CLOSE; #332 Q5, Alice's choice (a) of 09-30). A block the native
/// reader could not have produced is an error, never a silent skip.
fn classify_unproven(block: Option<&OwnerEpochBlock>) -> GlobalLedgerResult<UnprovenReason> {
    let Some(block) = block else {
        return Ok(UnprovenReason::ProofMissing);
    };
    if block.legacy {
        return Ok(UnprovenReason::LegacyJournal);
    }
    match (block.last_active, block.last_disposition) {
        // A last record that still declares resources in use or unconfirmed reaches this
        // start only after OwnerGuard found its process gone (`release_exited_owner`):
        // released by process exit, like an epoch that ends in a normal exit record.
        (true, Some(OwnerResourceDisposition::InUse | OwnerResourceDisposition::Unconfirmed))
        | (false, Some(OwnerResourceDisposition::None))
            if block.consistent && !block.confirmed_closed =>
        {
            Ok(UnprovenReason::ProcessExitOnly)
        }
        (true, Some(OwnerResourceDisposition::InUse | OwnerResourceDisposition::Unconfirmed))
        | (false, Some(OwnerResourceDisposition::None))
        | (
            true,
            Some(OwnerResourceDisposition::None | OwnerResourceDisposition::ConfirmedClosed),
        ) => Ok(UnprovenReason::ProofMissing),
        _ => Err(invalid(LedgerCode::PriorEpochUnprovenUnclassified.code())),
    }
}

/// LN-7 (#422, LEAN-L §3.5): what the start knows of one prior owner's import, checked against
/// the native read once per owner and page, not once per scope.
enum PriorOwner {
    /// Not sealed, and the owner's epoch has no known end (or was reopened): none of its scopes
    /// closes.
    Unbounded,
    /// Sealed and still supported by the native read: its scopes close up to `upper`.
    Sealed { upper: u64 },
    /// Not sealed yet: the import the owner's first open scope writes.
    Unsealed(Box<PriorEpochOwnerImport>),
}

impl<B: super::super::storage::DurableStorage> super::super::storage::EventStore<B> {
    /// The page counts visited scopes, including unknown/already closed scopes. It only
    /// appends facts; retention material operations retain their original admission path.
    /// A prior owner without a native proof is imported and closed with an explicit
    /// unproven reason; a sealed import must still be supported by the native read.
    /// LN-7 (#422, LEAN-L §3.5): only the scopes of the owners `prior_epoch_owners` names are
    /// visited: the previous epoch's, and those whose closes the previous start began.
    pub(in crate::global) fn reconcile_prior_epoch_closes(
        &mut self,
        writer: OwnerEpoch,
        journal: &RuntimeOwnerJournal,
        after: Option<PriorEpochScope>,
        deadline: Instant,
    ) -> GlobalLedgerResult<(Option<PriorEpochScope>, Vec<PersistedEvent>)> {
        deadline_check(deadline)?;
        if self.recovering_retention
            || self.retention.has_pending()
            || self.retention.owner_at(self.retention.through_sequence) != Some(writer)
            || journal
                .last()
                .is_some_and(|last| last.owner_epoch == writer)
        {
            return Err(invalid(LedgerCode::PriorEpochStartupBarrierConflict.code()));
        }
        let owners = self.retention.prior_epoch_owners(writer);
        let (page, next) = self.retention.prior_epoch_page(&owners, after);
        let mut appended = Vec::new();
        let mut checked = BTreeMap::<OwnerEpoch, PriorOwner>::new();
        for (scope, (scope_source, last)) in page {
            deadline_check(deadline)?;
            let completed = self.retention.prior_epoch_closes.contains_key(&scope);
            if !completed
                && (self.retention.ordinary_close(&scope).is_some()
                    || self.client_action_scope(&scope_source, last)?)
            {
                continue;
            }
            let state = match checked.remove(&scope.owner) {
                Some(state) => state,
                None => self.prior_owner(scope.owner, journal)?,
            };
            let state = match state {
                PriorOwner::Unbounded => {
                    checked.insert(scope.owner, PriorOwner::Unbounded);
                    continue;
                }
                PriorOwner::Sealed { upper } => {
                    if last > upper {
                        return Err(invalid(LedgerCode::PriorEpochNativeProofConflict.code()));
                    }
                    PriorOwner::Sealed { upper }
                }
                PriorOwner::Unsealed(record) => {
                    let upper = record.scope_upper_sequence;
                    if last > upper {
                        return Err(invalid(LedgerCode::PriorEpochScopeRangeConflict.code()));
                    }
                    appended.push(self.append_prior_epoch_fact(
                        writer,
                        PriorEpochCloseFact::OwnerImported(*record),
                        deadline,
                    )?);
                    PriorOwner::Sealed { upper }
                }
            };
            checked.insert(scope.owner, state);
            if completed {
                continue;
            }
            let (proof_source, proof) = self
                .retention
                .owner_imports
                .get(&scope.owner)
                .expect("sealed import");
            if last > proof.scope_upper_sequence {
                return Err(invalid(LedgerCode::PriorEpochScopeRangeConflict.code()));
            }
            let close = PriorEpochScopeClose {
                subject: scope.subject(),
                proof: *proof_source,
                scope_source,
                through_sequence: proof.through_sequence,
                scope_upper_sequence: proof.scope_upper_sequence,
                basis: proof.basis,
            };
            appended.push(self.append_prior_epoch_fact(
                writer,
                PriorEpochCloseFact::ScopeClosed(close),
                deadline,
            )?);
        }
        deadline_check(deadline)?;
        Ok((next, appended))
    }

    /// #411 P51: whether the scope whose first record is `source` and whose last record is at
    /// `last` holds one record only, a `client.action`. Such a record traces an action outside
    /// the Runtime (an MCP or Lab call, a button): its request records nothing else, so its scope
    /// holds no resource and no capture and no start closes it. Its one record is read; every
    /// other scope is closed as before. The scope stays among the scope sources, so the closes
    /// earlier starts wrote for such scopes still check.
    fn client_action_scope(&self, source: &TerminalEvent, last: u64) -> GlobalLedgerResult<bool> {
        if source.sequence != last {
            return Ok(false);
        }
        Ok(self
            .history()
            .record_at(source)?
            .is_some_and(|event| event.event_type() == EventType::ClientAction))
    }

    /// LN-7: the import of prior owner `owner`, checked once: a sealed import must still be
    /// supported by the native read and seal the owner's epoch upper sequence; an owner without
    /// one gets one, built here, when its first open scope closes.
    fn prior_owner(
        &self,
        owner: OwnerEpoch,
        journal: &RuntimeOwnerJournal,
    ) -> GlobalLedgerResult<PriorOwner> {
        let sealed = self.retention.owner_imports.get(&owner);
        let native = journal.proofs.get(&owner);
        if native.is_none()
            && sealed.is_some_and(|(_, proof)| proof.basis == PriorEpochCloseBasis::Proven)
        {
            return Err(invalid(LedgerCode::PriorEpochNativeProofMissing.code()));
        }
        let current = match native {
            Some(_) => PriorEpochCloseBasis::Proven,
            None => PriorEpochCloseBasis::Unproven {
                reason: classify_unproven(journal.block(owner))?,
            },
        };
        let upper = self.retention.prior_epoch_upper(owner);
        if let Some((_, proof)) = sealed {
            let supported = match proof.basis {
                PriorEpochCloseBasis::Proven => journal.supports(&proof.evidence),
                PriorEpochCloseBasis::Unproven { .. } => {
                    current == proof.basis && journal.supports_observation(&proof.evidence)
                }
            };
            if !supported || upper != Some(proof.scope_upper_sequence) {
                return Err(invalid(LedgerCode::PriorEpochNativeProofConflict.code()));
            }
            return Ok(PriorOwner::Sealed {
                upper: proof.scope_upper_sequence,
            });
        }
        let Some(upper) = upper else {
            return Ok(PriorOwner::Unbounded);
        };
        Ok(PriorOwner::Unsealed(Box::new(PriorEpochOwnerImport {
            evidence: match native {
                Some(proof) => proof.clone(),
                None => journal.observation(owner),
            },
            basis: current,
            through_sequence: self.retention.through_sequence,
            scope_upper_sequence: upper,
        })))
    }

    fn append_prior_epoch_fact(
        &mut self,
        writer: OwnerEpoch,
        fact: PriorEpochCloseFact,
        deadline: Instant,
    ) -> GlobalLedgerResult<PersistedEvent> {
        use actingcommand_contract::{
            EventActor, EventDraft, EventLinksDraft, EventOrigin, IdentifierIssuer, OriginModule,
            RuntimePayloadDraft,
        };
        deadline_check(deadline)?;
        let issue = || invalid(LedgerCode::PriorEpochIdentifierFailed.code());
        let ids = IdentifierIssuer::new().map_err(|_| issue())?;
        let links = EventLinksDraft::default()
            .with_request_id(ids.mint_request_id().map_err(|_| issue())?)
            .with_correlation_id(ids.mint_correlation_id().map_err(|_| issue())?)
            .with_action_id(ids.mint_action_id().map_err(|_| issue())?);
        let draft = EventDraft::new(
            ids.mint_event_id().map_err(|_| issue())?,
            retention_now()?,
            EventSeverity::Info,
            EventOrigin::new(
                EventSource::Runtime,
                OriginModule::GlobalLedger,
                EventActor::Runtime,
            ),
            links,
            RuntimePayloadDraft::prior_epoch_close(writer, fact).into(),
        )
        .sanitize(&super::super::Sha256SecretFingerprinter::new(
            b"actingcommand-ledger-owner-close-v1",
        )?)
        .map_err(|_| invalid(LedgerCode::PriorEpochSanitizeFailed.code()))?;
        let event = PersistedEvent::from_sanitized(self.next_sequence, draft)
            .map_err(|error| invalid(error.code()))?;
        deadline_check(deadline)?;
        self.persist_retention_checked(event, true)
    }
}

impl super::super::GlobalLedger {
    /// Startup only: the native capability comes from OwnerGuard's complete locked read.
    /// On any error the caller stops startup; a timed-out append is never resubmitted.
    pub fn reconcile_prior_epoch_closes(
        &self,
        writer: OwnerEpoch,
        journal: RuntimeOwnerJournal,
        deadline: Instant,
    ) -> GlobalLedgerResult<()> {
        let journal = Arc::new(journal);
        let mut after = None;
        loop {
            deadline_check(deadline)?;
            let (response, receiver) = std::sync::mpsc::sync_channel(1);
            let sender = self
                .sender
                .as_ref()
                .ok_or_else(|| invalid(LedgerCode::WriterUnavailable.code()))?;
            super::super::send_command(
                sender,
                super::super::WriterCommand::ReconcilePriorEpochCloses {
                    writer,
                    journal: Arc::clone(&journal),
                    after,
                    deadline,
                    response,
                },
                LedgerLocation::ReconcilePriorEpochCloses.location(),
            )?;
            let remaining = deadline
                .saturating_duration_since(Instant::now())
                .min(self.reply_timeout);
            let next = receiver.recv_timeout(remaining).map_err(|error| {
                invalid(match error {
                    std::sync::mpsc::RecvTimeoutError::Timeout => {
                        LedgerCode::WriterResponseTimeout.code()
                    }
                    std::sync::mpsc::RecvTimeoutError::Disconnected => {
                        LedgerCode::WriterUnavailable.code()
                    }
                })
            })??;
            match next {
                Some(value) if after.is_none_or(|previous| value > previous) => after = Some(value),
                Some(_) => return Err(invalid(LedgerCode::PriorEpochCursorConflict.code())),
                None => return Ok(()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RetentionIndex;
    use crate::PersistedEvent;
    use crate::global::Sha256SecretFingerprinter;
    use actingcommand_contract::{
        AuditInput, EventAction, EventActor, EventDraft, EventLinksDraft, EventOrigin,
        EventPayloadDraft, EventSeverity, EventSource, IdentifierIssuer, OriginModule, OwnerEpoch,
        RunId, RuntimeLifecyclePhase, RuntimePayloadDraft, TaskPayloadDraft,
    };
    use std::collections::BTreeSet;

    /// Records of several owner epochs, numbered from 1 in the order they are added.
    struct History {
        ids: IdentifierIssuer,
        fingerprinter: Sha256SecretFingerprinter,
        events: Vec<PersistedEvent>,
    }

    impl History {
        fn push(&mut self, links: EventLinksDraft, payload: EventPayloadDraft) {
            let sequence = self.events.len() as u64 + 1;
            let draft = EventDraft::new(
                self.ids.mint_event_id().expect("event id"),
                1_752_147_200_000 + sequence,
                EventSeverity::Info,
                EventOrigin::new(
                    EventSource::Runtime,
                    OriginModule::Runtime,
                    EventActor::Runtime,
                ),
                links,
                payload,
            )
            .sanitize(&self.fingerprinter)
            .expect("sanitize");
            self.events
                .push(PersistedEvent::from_sanitized(sequence, draft).expect("persisted event"));
        }

        /// A record of `owner`'s epoch; the first one after another owner's starts the epoch.
        fn owner(&mut self, owner: OwnerEpoch) {
            self.push(
                EventLinksDraft::default(),
                RuntimePayloadDraft::lifecycle_observed(
                    owner,
                    RuntimeLifecyclePhase::ShutdownRequested,
                    AuditInput::new(),
                )
                .into(),
            );
        }

        /// A run's first record in the current epoch: its scope opens and nothing closes it.
        fn open_run(&mut self) -> RunId {
            let run = self.ids.mint_run_id().expect("run");
            let id = *run.transport();
            let links = EventLinksDraft::default()
                .with_instance_id(self.ids.mint_instance_id().expect("instance"))
                .with_request_id(self.ids.mint_request_id().expect("request"))
                .with_correlation_id(self.ids.mint_correlation_id().expect("correlation"))
                .with_task_id(self.ids.mint_task_id().expect("task"))
                .with_run_id(run);
            self.push(
                links,
                TaskPayloadDraft::started(EventAction::RuntimeTaskRun, AuditInput::new()).into(),
            );
            id
        }
    }

    /// LN-7 (#422, LEAN-L §3.5; the LN-7 row): the start's prior-epoch closes come from the
    /// previous epoch only. An older epoch and the previous epoch each leave two runs' scopes
    /// open; the writer's start visits the previous epoch's scopes, every one of them, and none
    /// of the older epoch's.
    #[test]
    fn prior_epoch_closes_come_from_the_previous_epoch_only() {
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        let [older, previous, writer] =
            [(); 3].map(|()| *ids.mint_owner_epoch().expect("owner epoch").transport());
        let mut history = History {
            ids,
            fingerprinter: Sha256SecretFingerprinter::new(b"ledger-ln7-prior-epoch")
                .expect("fingerprinter"),
            events: Vec::new(),
        };
        history.owner(older);
        let older_runs = [history.open_run(), history.open_run()];
        history.owner(older);
        history.owner(previous);
        let previous_runs = [history.open_run(), history.open_run()];
        history.owner(previous);
        history.owner(writer);
        let index = RetentionIndex::from_events(&history.events).expect("retention index");

        let owners = index.prior_epoch_owners(writer);
        assert_eq!(owners, BTreeSet::from([previous]));
        let (page, next) = index.prior_epoch_page(&owners, None);
        assert_eq!(next, None);
        let visited = page
            .iter()
            .map(|(scope, _)| (scope.owner, scope.run))
            .collect::<BTreeSet<_>>();
        assert_eq!(
            visited,
            previous_runs
                .iter()
                .map(|run| (previous, Some(*run)))
                .collect::<BTreeSet<_>>()
        );
        assert!(
            page.iter().all(|(scope, _)| scope.owner != older
                && scope.run.is_none_or(|run| !older_runs.contains(&run))),
            "no scope of the older epoch is visited"
        );
    }
}
