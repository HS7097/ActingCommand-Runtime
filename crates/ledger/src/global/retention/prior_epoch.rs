// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::owner_journal::RuntimeOwnerJournal;
use actingcommand_contract::{
    PriorEpochCloseFact, PriorEpochOwnerImport, PriorEpochScope, PriorEpochScopeClose,
};
use std::{sync::Arc, time::Instant};

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

    pub(super) fn validate_prior_epoch<E: LedgerEventRead>(
        &self,
        event: &E,
        events: &[E],
        guarded: bool,
    ) -> GlobalLedgerResult<()> {
        let Some(observation) = fact(event) else {
            return Ok(());
        };
        let writer = recorded_owner(event).ok_or_else(|| invalid("prior_epoch_writer_missing"))?;
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
            return Err(invalid("prior_epoch_close_admission_conflict"));
        }
        observation
            .validate(writer)
            .map_err(|_| invalid("prior_epoch_close_invalid"))?;
        match observation {
            PriorEpochCloseFact::OwnerImported(record) => {
                if record.through_sequence != self.through_sequence
                    || self.prior_epoch_upper(record.evidence.subject)
                        != Some(record.scope_upper_sequence)
                    || self.owner_imports.contains_key(&record.evidence.subject)
                {
                    return Err(invalid("prior_epoch_import_prefix_conflict"));
                }
            }
            PriorEpochCloseFact::ScopeClosed(record) => {
                let scope = ClosureScope::from_subject(record.subject);
                let Some((proof_source, proof)) = self.owner_imports.get(&scope.owner) else {
                    return Err(invalid("prior_epoch_import_missing"));
                };
                if *proof_source != record.proof
                    || proof.through_sequence != record.through_sequence
                    || proof.scope_upper_sequence != record.scope_upper_sequence
                    || self.prior_epoch_upper(scope.owner) != Some(record.scope_upper_sequence)
                    || self.scope_sources.get(&scope).is_none_or(|(first, last)| {
                        *first != record.scope_source || *last > record.scope_upper_sequence
                    })
                    || self.ordinary_close(&scope).is_some()
                    || self.prior_epoch_closes.contains_key(&scope)
                {
                    return Err(invalid("prior_epoch_scope_prefix_conflict"));
                }
                let original = source(events, &record.scope_source)?;
                if self.owner_at(original.sequence()) != Some(scope.owner)
                    || ClosureScope::from_event(original, scope.owner) != Some(scope)
                {
                    return Err(invalid("prior_epoch_scope_source_conflict"));
                }
                let imported = source(events, &record.proof)?;
                if !matches!(fact(imported), Some(PriorEpochCloseFact::OwnerImported(value)) if value == proof)
                {
                    return Err(invalid("prior_epoch_import_source_conflict"));
                }
            }
        }
        Ok(())
    }

    pub(super) fn apply_prior_epoch<E: LedgerEventRead>(&mut self, event: &E) {
        match fact(event) {
            Some(PriorEpochCloseFact::OwnerImported(record)) => {
                self.owner_imports
                    .insert(record.evidence.subject, (terminal(event), record.clone()));
            }
            Some(PriorEpochCloseFact::ScopeClosed(record)) => {
                self.prior_epoch_closes.insert(
                    ClosureScope::from_subject(record.subject),
                    (terminal(event), record.clone()),
                );
            }
            None => {}
        }
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
        return Err(invalid("prior_epoch_close_deadline"));
    }
    Ok(())
}

impl<B: super::super::storage::DurableStorage> super::super::storage::EventStore<B> {
    /// The page counts visited scopes, including unknown/already closed scopes. It only
    /// appends facts; retention material operations retain their original admission path.
    pub(in crate::global) fn reconcile_prior_epoch_closes(
        &mut self,
        writer: OwnerEpoch,
        journal: &RuntimeOwnerJournal,
        after: Option<PriorEpochScope>,
        deadline: Instant,
    ) -> GlobalLedgerResult<(Option<PriorEpochScope>, Vec<PersistedEvent>)> {
        use std::ops::Bound::{Excluded, Unbounded};
        deadline_check(deadline)?;
        if self.recovering_retention
            || self.retention.has_pending()
            || self.retention.owner_at(self.retention.through_sequence) != Some(writer)
            || journal
                .last()
                .is_some_and(|last| last.owner_epoch == writer)
        {
            return Err(invalid("prior_epoch_startup_barrier_conflict"));
        }
        let page = self
            .retention
            .scope_sources
            .range((
                after
                    .map(ClosureScope::from_subject)
                    .map_or(Unbounded, Excluded),
                Unbounded,
            ))
            .take(RETENTION_ROUND_OBJECTS)
            .map(|(scope, value)| (*scope, *value))
            .collect::<Vec<_>>();
        let next = (page.len() == RETENTION_ROUND_OBJECTS)
            .then(|| page.last().expect("full page").0.subject());
        let mut appended = Vec::new();
        for (scope, (scope_source, last)) in page {
            deadline_check(deadline)?;
            if scope.owner == writer {
                continue;
            }
            let completed = self.retention.prior_epoch_closes.get(&scope);
            if completed.is_none() && self.retention.ordinary_close(&scope).is_some() {
                continue;
            }
            let sealed = self.retention.owner_imports.get(&scope.owner);
            let Some(native) = journal.proofs.get(&scope.owner) else {
                if sealed.is_some() {
                    return Err(invalid("prior_epoch_native_proof_missing"));
                }
                continue;
            };
            if let Some((_, proof)) = sealed {
                let original = &proof.evidence;
                if !journal.supports(original)
                    || self.retention.prior_epoch_upper(scope.owner)
                        != Some(proof.scope_upper_sequence)
                    || last > proof.scope_upper_sequence
                {
                    return Err(invalid("prior_epoch_native_proof_conflict"));
                }
                if completed.is_some() {
                    continue;
                }
            } else {
                let Some(upper) = self.retention.prior_epoch_upper(scope.owner) else {
                    continue;
                };
                if last > upper {
                    return Err(invalid("prior_epoch_scope_range_conflict"));
                }
                let record = PriorEpochOwnerImport {
                    evidence: native.clone(),
                    through_sequence: self.retention.through_sequence,
                    scope_upper_sequence: upper,
                };
                appended.push(self.append_prior_epoch_fact(
                    writer,
                    PriorEpochCloseFact::OwnerImported(record),
                    deadline,
                )?);
            }
            let (proof_source, proof) = self
                .retention
                .owner_imports
                .get(&scope.owner)
                .expect("sealed import");
            if last > proof.scope_upper_sequence {
                return Err(invalid("prior_epoch_scope_range_conflict"));
            }
            let close = PriorEpochScopeClose {
                subject: scope.subject(),
                proof: *proof_source,
                scope_source,
                through_sequence: proof.through_sequence,
                scope_upper_sequence: proof.scope_upper_sequence,
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
        let issue = || invalid("prior_epoch_identifier_failed");
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
        .map_err(|_| invalid("prior_epoch_sanitize_failed"))?;
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
                .ok_or_else(|| invalid("writer_unavailable"))?;
            super::super::send_command(
                sender,
                super::super::WriterCommand::ReconcilePriorEpochCloses {
                    writer,
                    journal: Arc::clone(&journal),
                    after,
                    deadline,
                    response,
                },
                "reconcile_prior_epoch_closes",
            )?;
            let remaining = deadline
                .saturating_duration_since(Instant::now())
                .min(super::super::COMMAND_TIMEOUT);
            let next = receiver.recv_timeout(remaining).map_err(|error| {
                invalid(match error {
                    std::sync::mpsc::RecvTimeoutError::Timeout => "writer_response_timeout",
                    std::sync::mpsc::RecvTimeoutError::Disconnected => "writer_unavailable",
                })
            })??;
            match next {
                Some(value) if after.is_none_or(|previous| value > previous) => after = Some(value),
                Some(_) => return Err(invalid("prior_epoch_cursor_conflict")),
                None => return Ok(()),
            }
        }
    }
}
