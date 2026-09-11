// SPDX-License-Identifier: AGPL-3.0-only

//! Private durable-store boundary. See contracts/ledger-store.md for the S0 contract.

use super::storage::{DurableStorage, EventStore};
use super::{CommitStatistics, GlobalLedgerResult};
use crate::PersistedEvent;
use actingcommand_contract::{
    EventQuery, PolicyExecutionEventData, ProjectionProfile, RuntimeEventQueryPage,
    RuntimeEventQueryPageRequest, SanitizedEventDraft,
};
use std::sync::Arc;

/// One opened, recovered store moves to the existing GlobalLedger writer.
/// Opening (including artifact verification) finishes before this transfer.
/// Reads address the verified committed snapshot and do no fallible storage I/O.
/// Projection, subscriptions and public request validation belong to GlobalLedger.
pub(super) trait LedgerStore: Send + 'static {
    fn commit_statistics(&self) -> Arc<CommitStatistics>;

    /// Success means durable persistence, index visibility and commit accounting.
    /// An error does not prove that no bytes or facts were committed.
    fn append(&mut self, draft: SanitizedEventDraft) -> GlobalLedgerResult<PersistedEvent>;

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
    fn commit_statistics(&self) -> Arc<CommitStatistics> {
        Arc::clone(&self.commit_statistics)
    }

    fn append(&mut self, draft: SanitizedEventDraft) -> GlobalLedgerResult<PersistedEvent> {
        Self::append(self, draft)
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
    ) -> GlobalLedgerResult<RuntimeEventQueryPage> {
        Self::project_view_page(self, query, profile, request)
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
