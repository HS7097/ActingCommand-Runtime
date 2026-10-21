// SPDX-License-Identifier: AGPL-3.0-only

//! Private durable-store boundary. See contracts/ledger-store.md for the S0 contract.

use super::capture::Captured;
use super::flush::FlushReport;
use super::read_work::ReadWork;
use super::storage::{DurableStorage, EventStore};
use super::{CommitStatistics, GlobalLedgerResult, LedgerProjectViewObservation};
use crate::PersistedEvent;
use actingcommand_contract::outcome::Location;
use actingcommand_contract::{
    EventId, EventQuery, IdentifierIssuer, PolicyExecutionEventData, ProjectionProfile,
    RuntimeEventQueryPage, RuntimeEventQueryPageRequest,
};
use std::sync::Arc;

/// One opened, recovered store moves to the existing GlobalLedger writer.
/// Opening (including artifact verification) finishes before this transfer.
/// Reads address committed facts. SQLite view and artifact-reference reads authenticate
/// their physical snapshot through the existing database owner before returning.
/// Projection, subscriptions and public request validation belong to GlobalLedger.
pub(super) trait LedgerStore: Send + 'static {
    fn reconcile_prior_epoch_closes(
        &mut self,
        writer: actingcommand_contract::OwnerEpoch,
        journal: &crate::owner_journal::RuntimeOwnerJournal,
        after: Option<actingcommand_contract::PriorEpochScope>,
        deadline: std::time::Instant,
    ) -> GlobalLedgerResult<(
        Option<actingcommand_contract::PriorEpochScope>,
        Vec<PersistedEvent>,
    )>;
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
    ) -> GlobalLedgerResult<super::ArtifactRetentionCandidates>;
    fn frame_retention_view(
        &self,
        now_unix_ms: u64,
        switches: super::FrameRetentionSwitches,
        work: &ReadWork,
    ) -> GlobalLedgerResult<super::FrameRetentionView>;
    fn admit_artifact_eviction(
        &mut self,
        guard: actingcommand_artifact_store::ArtifactDeleteGuard,
    ) -> GlobalLedgerResult<(super::ArtifactEvictionAdmission, Vec<PersistedEvent>)>;
    fn finish_artifact_eviction(
        &mut self,
        permit: super::ArtifactEvictionPermit,
        disposition: actingcommand_contract::ArtifactEvictionDisposition,
        outcome: Option<actingcommand_contract::outcome::Outcome>,
    ) -> GlobalLedgerResult<PersistedEvent>;
    fn commit_statistics(&self) -> Arc<CommitStatistics>;

    /// Workflow #383 AO-2 (`contracts/ledger-observer.md` §8): commits one batch the flush
    /// thread took from the capture queue, in capture order. A committed record means durable
    /// persistence, index visibility and commit accounting; an error does not prove that no
    /// bytes or facts were committed.
    fn commit_captured(
        &mut self,
        batch: Vec<Captured>,
        identifiers: &IdentifierIssuer,
    ) -> FlushReport;
    /// Workflow #383 AO-2 (§8.4): the ledger sequence of a committed record, for the barrier.
    fn committed_sequence(&self, event_id: &EventId) -> Option<u64>;

    /// Revalidates persisted admission/effect/release facts. Returns the existing
    /// or new completion, plus only newly appended events in sequence order.
    fn reconcile_scheduled_policy_settlement(
        &mut self,
        execution: PolicyExecutionEventData,
    ) -> GlobalLedgerResult<(PersistedEvent, Vec<PersistedEvent>)>;

    fn query(&self, query: &EventQuery) -> Vec<PersistedEvent>;
    /// #411 A51 (R2): every read below stops where `work`'s budget runs out; a page answers
    /// incomplete, any other read `ledger_read_budget_exceeded` of `operation`.
    fn query_within(
        &self,
        query: &EventQuery,
        work: &ReadWork,
        operation: Location,
    ) -> GlobalLedgerResult<Vec<PersistedEvent>>;
    /// The page after `range.0` through `range.1`.
    fn query_page(
        &self,
        query: &EventQuery,
        range: (u64, u64),
        page_events: usize,
        work: &ReadWork,
        operation: Location,
    ) -> GlobalLedgerResult<Vec<PersistedEvent>>;
    fn latest_sequence(&self) -> u64;
    fn project_view_page(
        &self,
        query: &EventQuery,
        profile: ProjectionProfile,
        request: &RuntimeEventQueryPageRequest,
        observation: &mut Option<LedgerProjectViewObservation>,
        work: &ReadWork,
    ) -> GlobalLedgerResult<RuntimeEventQueryPage>;
    /// The replay page after `range.0` through `range.1`.
    fn replay_page(
        &self,
        range: (u64, u64),
        page_events: usize,
        work: &ReadWork,
        operation: Location,
    ) -> GlobalLedgerResult<Vec<PersistedEvent>>;
    /// #411 A51: the root a test hook names this store by (its canonical state root).
    #[cfg(any(test, feature = "test-hooks"))]
    fn hook_root(&self) -> std::path::PathBuf;
    fn close(self) -> GlobalLedgerResult<()>;
}

impl<B: DurableStorage> LedgerStore for EventStore<B> {
    fn reconcile_prior_epoch_closes(
        &mut self,
        writer: actingcommand_contract::OwnerEpoch,
        journal: &crate::owner_journal::RuntimeOwnerJournal,
        after: Option<actingcommand_contract::PriorEpochScope>,
        deadline: std::time::Instant,
    ) -> GlobalLedgerResult<(
        Option<actingcommand_contract::PriorEpochScope>,
        Vec<PersistedEvent>,
    )> {
        Self::reconcile_prior_epoch_closes(self, writer, journal, after, deadline)
    }
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
    ) -> GlobalLedgerResult<super::ArtifactRetentionCandidates> {
        Self::retention_candidates(self, after)
    }
    fn frame_retention_view(
        &self,
        now_unix_ms: u64,
        switches: super::FrameRetentionSwitches,
        work: &ReadWork,
    ) -> GlobalLedgerResult<super::FrameRetentionView> {
        Self::frame_retention_view(self, now_unix_ms, switches, work)
    }
    fn admit_artifact_eviction(
        &mut self,
        guard: actingcommand_artifact_store::ArtifactDeleteGuard,
    ) -> GlobalLedgerResult<(super::ArtifactEvictionAdmission, Vec<PersistedEvent>)> {
        Self::admit_artifact_eviction(self, guard)
    }
    fn finish_artifact_eviction(
        &mut self,
        permit: super::ArtifactEvictionPermit,
        disposition: actingcommand_contract::ArtifactEvictionDisposition,
        outcome: Option<actingcommand_contract::outcome::Outcome>,
    ) -> GlobalLedgerResult<PersistedEvent> {
        Self::finish_artifact_eviction(self, permit, disposition, outcome)
    }
    fn commit_statistics(&self) -> Arc<CommitStatistics> {
        Arc::clone(&self.commit_statistics)
    }

    fn commit_captured(
        &mut self,
        batch: Vec<Captured>,
        identifiers: &IdentifierIssuer,
    ) -> FlushReport {
        Self::commit_captured(self, batch, identifiers)
    }

    fn committed_sequence(&self, event_id: &EventId) -> Option<u64> {
        Self::committed_sequence(self, event_id)
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

    fn query_within(
        &self,
        query: &EventQuery,
        work: &ReadWork,
        operation: Location,
    ) -> GlobalLedgerResult<Vec<PersistedEvent>> {
        Self::query_within(self, query, work, operation)
    }

    fn query_page(
        &self,
        query: &EventQuery,
        range: (u64, u64),
        page_events: usize,
        work: &ReadWork,
        operation: Location,
    ) -> GlobalLedgerResult<Vec<PersistedEvent>> {
        Self::query_page_within(self, query, range, page_events, work, operation)
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
        work: &ReadWork,
    ) -> GlobalLedgerResult<RuntimeEventQueryPage> {
        if observation.is_some() {
            Self::project_view_page_observed(self, query, profile, request, observation, work)
        } else {
            Self::project_view_page(self, query, profile, request, work)
        }
    }

    fn replay_page(
        &self,
        range: (u64, u64),
        page_events: usize,
        work: &ReadWork,
        operation: Location,
    ) -> GlobalLedgerResult<Vec<PersistedEvent>> {
        Self::replay_page_within(self, range, page_events, work, operation)
    }

    #[cfg(any(test, feature = "test-hooks"))]
    fn hook_root(&self) -> std::path::PathBuf {
        // The gate names its root canonically (`\\?\` on Windows); so does the writer.
        let root = self.backend.material_root();
        root.canonicalize().unwrap_or_else(|_| root.to_path_buf())
    }

    fn close(self) -> GlobalLedgerResult<()> {
        Self::close(self)
    }
}
