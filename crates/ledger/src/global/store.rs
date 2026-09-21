// SPDX-License-Identifier: AGPL-3.0-only

//! Private durable-store boundary. See contracts/ledger-store.md for the S0 contract.

use super::storage::{DurableStorage, EventStore};
use super::{
    CommitStatistics, GlobalLedgerResult, LedgerAppendObservation, LedgerProjectViewObservation,
};
use crate::PersistedEvent;
use actingcommand_contract::{
    EventQuery, PolicyExecutionEventData, ProjectionProfile, RuntimeEventQueryPage,
    RuntimeEventQueryPageRequest, SanitizedEventDraft,
};
use std::sync::Arc;

/// One opened, recovered store moves to the existing GlobalLedger writer.
/// Opening (including artifact verification) finishes before this transfer.
/// Reads address committed facts. SQLite view and artifact-reference reads authenticate
/// their physical snapshot through the existing database owner before returning.
/// Projection, subscriptions and public request validation belong to GlobalLedger.
pub(super) trait LedgerStore: Send + 'static {
    fn release_lab_pin(
        &mut self,
        target: actingcommand_contract::LabPinReleaseTarget,
        request: actingcommand_contract::TerminalEvent,
    ) -> GlobalLedgerResult<(
        actingcommand_contract::LabPinReleaseResult,
        Vec<PersistedEvent>,
    )>;
    fn resolve_artifact(
        &self,
        selection: &super::LedgerArtifactSelection,
        deadline: std::time::Instant,
    ) -> GlobalLedgerResult<super::ResolvedLedgerArtifact>;
    fn retention_candidates(
        &self,
        after: Option<actingcommand_contract::ArtifactId>,
        policy: actingcommand_contract::FailedRunRetentionPolicy,
    ) -> GlobalLedgerResult<super::ArtifactRetentionCandidates>;
    fn admit_artifact_eviction(
        &mut self,
        guard: actingcommand_artifact_store::ArtifactDeleteGuard,
        policy: actingcommand_contract::FailedRunRetentionPolicy,
    ) -> GlobalLedgerResult<(super::ArtifactEvictionAdmission, Vec<PersistedEvent>)>;
    fn finish_artifact_eviction(
        &mut self,
        permit: super::ArtifactEvictionPermit,
        disposition: actingcommand_contract::ArtifactEvictionDisposition,
        io: Option<actingcommand_contract::ArtifactEvictionIo>,
    ) -> GlobalLedgerResult<PersistedEvent>;
    fn commit_statistics(&self) -> Arc<CommitStatistics>;

    /// Success means durable persistence, index visibility and commit accounting.
    /// An error does not prove that no bytes or facts were committed.
    fn append(
        &mut self,
        draft: SanitizedEventDraft,
        observation: &mut Option<LedgerAppendObservation>,
    ) -> GlobalLedgerResult<PersistedEvent>;
    fn append_transaction(
        &mut self,
        draft: SanitizedEventDraft,
        work: &dyn super::LedgerTransactionWork,
    ) -> GlobalLedgerResult<PersistedEvent>;

    /// Revalidates persisted admission/effect/release facts. Returns the existing
    /// or new completion, plus only newly appended events in sequence order.
    fn reconcile_scheduled_policy_settlement(
        &mut self,
        execution: PolicyExecutionEventData,
    ) -> GlobalLedgerResult<(PersistedEvent, Vec<PersistedEvent>)>;

    fn query(&self, query: &EventQuery) -> Vec<PersistedEvent>;
    fn query_page(
        &self,
        query: &EventQuery,
        after_sequence: u64,
        through_sequence: u64,
        page_events: usize,
    ) -> Vec<PersistedEvent>;
    fn latest_sequence(&self) -> u64;
    fn project_view_page(
        &self,
        query: &EventQuery,
        profile: ProjectionProfile,
        request: &RuntimeEventQueryPageRequest,
        observation: &mut Option<LedgerProjectViewObservation>,
    ) -> GlobalLedgerResult<RuntimeEventQueryPage>;
    fn replay_page(
        &self,
        after_sequence: u64,
        through_sequence: u64,
        page_events: usize,
    ) -> Vec<PersistedEvent>;
    fn close(self) -> GlobalLedgerResult<()>;
}

impl<B: DurableStorage> LedgerStore for EventStore<B> {
    fn release_lab_pin(
        &mut self,
        target: actingcommand_contract::LabPinReleaseTarget,
        request: actingcommand_contract::TerminalEvent,
    ) -> GlobalLedgerResult<(
        actingcommand_contract::LabPinReleaseResult,
        Vec<PersistedEvent>,
    )> {
        Self::release_lab_pin(self, target, request)
    }
    fn resolve_artifact(
        &self,
        selection: &super::LedgerArtifactSelection,
        deadline: std::time::Instant,
    ) -> GlobalLedgerResult<super::ResolvedLedgerArtifact> {
        Self::resolve_artifact(self, selection, deadline)
    }
    fn retention_candidates(
        &self,
        after: Option<actingcommand_contract::ArtifactId>,
        policy: actingcommand_contract::FailedRunRetentionPolicy,
    ) -> GlobalLedgerResult<super::ArtifactRetentionCandidates> {
        Self::retention_candidates(self, after, policy)
    }
    fn admit_artifact_eviction(
        &mut self,
        guard: actingcommand_artifact_store::ArtifactDeleteGuard,
        policy: actingcommand_contract::FailedRunRetentionPolicy,
    ) -> GlobalLedgerResult<(super::ArtifactEvictionAdmission, Vec<PersistedEvent>)> {
        Self::admit_artifact_eviction(self, guard, policy)
    }
    fn finish_artifact_eviction(
        &mut self,
        permit: super::ArtifactEvictionPermit,
        disposition: actingcommand_contract::ArtifactEvictionDisposition,
        io: Option<actingcommand_contract::ArtifactEvictionIo>,
    ) -> GlobalLedgerResult<PersistedEvent> {
        Self::finish_artifact_eviction(self, permit, disposition, io)
    }
    fn commit_statistics(&self) -> Arc<CommitStatistics> {
        Arc::clone(&self.commit_statistics)
    }

    fn append(
        &mut self,
        draft: SanitizedEventDraft,
        observation: &mut Option<LedgerAppendObservation>,
    ) -> GlobalLedgerResult<PersistedEvent> {
        if observation.is_some() {
            Self::append_observed(self, draft, observation)
        } else {
            Self::append(self, draft)
        }
    }

    fn append_transaction(
        &mut self,
        draft: SanitizedEventDraft,
        work: &dyn super::LedgerTransactionWork,
    ) -> GlobalLedgerResult<PersistedEvent> {
        Self::append_transaction(self, draft, work)
    }

    fn reconcile_scheduled_policy_settlement(
        &mut self,
        execution: PolicyExecutionEventData,
    ) -> GlobalLedgerResult<(PersistedEvent, Vec<PersistedEvent>)> {
        Self::reconcile_scheduled_policy_settlement(self, execution)
    }

    fn query(&self, query: &EventQuery) -> Vec<PersistedEvent> {
        Self::query(self, query)
    }

    fn query_page(
        &self,
        query: &EventQuery,
        after_sequence: u64,
        through_sequence: u64,
        page_events: usize,
    ) -> Vec<PersistedEvent> {
        Self::query_page(self, query, after_sequence, through_sequence, page_events)
    }

    fn latest_sequence(&self) -> u64 {
        Self::latest_sequence(self)
    }

    fn project_view_page(
        &self,
        query: &EventQuery,
        profile: ProjectionProfile,
        request: &RuntimeEventQueryPageRequest,
        observation: &mut Option<LedgerProjectViewObservation>,
    ) -> GlobalLedgerResult<RuntimeEventQueryPage> {
        if observation.is_some() {
            Self::project_view_page_observed(self, query, profile, request, observation)
        } else {
            Self::project_view_page(self, query, profile, request)
        }
    }

    fn replay_page(
        &self,
        after_sequence: u64,
        through_sequence: u64,
        page_events: usize,
    ) -> Vec<PersistedEvent> {
        Self::replay_page(self, after_sequence, through_sequence, page_events)
    }

    fn close(self) -> GlobalLedgerResult<()> {
        Self::close(self)
    }
}
