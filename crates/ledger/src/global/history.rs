// SPDX-License-Identifier: AGPL-3.0-only

//! LN-5 (#422, LEAN-L §3.3, §3.4): the ledger's history is on disk. The writer keeps no list of
//! the records and no index of them: a read of the history reads the store (SQLite, each row
//! authenticated on its own) one page of `ledger_observer.history_page_rows` rows at a time, on
//! the writer's thread and within its read budget; a check an append makes of earlier records is
//! an indexed lookup of the store and of the records its own batch has staged.

use super::projection::{LabKeys, is_lab_anchor, query_matches_fields};
use super::read_work::ReadWork;
use super::retention::RetentionIndex;
use super::storage::{DurableStorage, EventStore};
use super::{GlobalLedgerResult, projection::EventIndexes};
use crate::PersistedEvent;
use crate::fact::LedgerEventRead;
use actingcommand_contract::{
    CorrelationId, EventId, EventPayload, EventQuery, LedgerView, PolicyPayload, RequestId, RunId,
    TerminalEvent,
};
use std::collections::BTreeSet;

/// The default of `ledger_observer.history_page_rows`: the rows of one page of a history read.
pub(super) const DEFAULT_HISTORY_PAGE_ROWS: usize = 256;
/// The largest `ledger_observer.history_page_rows`.
pub(super) const MAX_HISTORY_PAGE_ROWS: usize = 4096;

/// What a history read selects: the records `query` names and, when `decision_id` is set, only
/// the policy dispatch and execution records of that decision. The store selects by its indexes
/// (the sequence key; the event type, origin, time and severity indexes; the link indexes; a
/// decision through the record's payload); the reader checks every selected record against the
/// whole selection again (`selects`), and against the query's view.
#[derive(Clone, Copy)]
pub(super) struct HistorySelection<'a> {
    pub(super) query: &'a EventQuery,
    pub(super) decision_id: Option<&'a str>,
}

impl<'a> HistorySelection<'a> {
    pub(super) const fn of(query: &'a EventQuery) -> Self {
        Self {
            query,
            decision_id: None,
        }
    }

    /// Whether `event` meets every condition of the selection, the query's view aside.
    pub(super) fn selects<E: LedgerEventRead>(&self, event: &E) -> bool {
        query_matches_fields(self.query, event)
            && self
                .decision_id
                .is_none_or(|decision| decision_of(event) == Some(decision))
    }
}

/// The decision a policy dispatch or execution record names.
pub(super) fn decision_of<E: LedgerEventRead>(event: &E) -> Option<&str> {
    match event.payload() {
        EventPayload::Policy(
            PolicyPayload::DispatchIntent(payload)
            | PolicyPayload::DispatchAdmitted(payload)
            | PolicyPayload::DispatchRejected(payload)
            | PolicyPayload::DispatchCompleted(payload),
        ) => Some(payload.decision_id()),
        EventPayload::Policy(PolicyPayload::ExecutionRecorded(payload)) => {
            Some(payload.decision_id())
        }
        _ => None,
    }
}

/// LN-5: the Lab relations of the history through one position (`LAB_RELATIONS`): the requests
/// and the correlations a Lab anchor names, and the runs one of whose records names one of them.
/// A record is related when its request, its correlation or its run is among them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct LabRelations {
    pub(super) requests: BTreeSet<RequestId>,
    pub(super) correlations: BTreeSet<CorrelationId>,
    pub(super) runs: BTreeSet<RunId>,
}

impl LabRelations {
    pub(super) fn related(&self, keys: &LabKeys) -> bool {
        keys.request
            .is_some_and(|request| self.requests.contains(&request))
            || keys
                .correlation
                .is_some_and(|correlation| self.correlations.contains(&correlation))
            || keys.run.is_some_and(|run| self.runs.contains(&run))
    }

    /// Adds the requests and correlations of the anchors among `events` at or before `through`.
    pub(super) fn add_anchors<E: LedgerEventRead>(&mut self, events: &[E], through: u64) {
        for event in events
            .iter()
            .filter(|event| event.sequence() <= through && is_lab_anchor(*event))
        {
            self.requests.extend(event.links().request_id().copied());
            self.correlations
                .extend(event.links().correlation_id().copied());
        }
    }

    /// Adds the runs of the records among `events` at or before `through` that name a request
    /// or a correlation of these relations.
    pub(super) fn add_runs<E: LedgerEventRead>(&mut self, events: &[E], through: u64) {
        for event in events.iter().filter(|event| event.sequence() <= through) {
            let links = event.links();
            if let Some(run) = links.run_id()
                && (links
                    .request_id()
                    .is_some_and(|request| self.requests.contains(request))
                    || links
                        .correlation_id()
                        .is_some_and(|correlation| self.correlations.contains(correlation)))
            {
                self.runs.insert(*run);
            }
        }
    }
}

/// LN-5 (LEAN-L §3.4): what retention reads of the history before the record it decides: a
/// record by its position, and whether a Lab anchor relates to a record. A full build answers
/// from its own records (`PriorRecords`); the writer from the store and its staged batch
/// (`WriterHistory`).
pub(super) trait RetentionHistory {
    type Event: LedgerEventRead;

    /// The record `reference` names (its sequence and its event id), if the history holds it.
    fn record(&self, reference: &TerminalEvent) -> GlobalLedgerResult<Option<Self::Event>>;

    /// Whether a Lab anchor at or before `through` relates to a record with `keys`.
    fn lab_related(&self, keys: &LabKeys, through: u64) -> GlobalLedgerResult<bool>;
}

/// The records before the one a full build decides, with their indexes.
pub(super) struct PriorRecords<'a, E> {
    pub(super) events: &'a [E],
    pub(super) indexes: &'a EventIndexes,
}

impl<E: LedgerEventRead> RetentionHistory for PriorRecords<'_, E> {
    type Event = E;

    fn record(&self, reference: &TerminalEvent) -> GlobalLedgerResult<Option<E>> {
        let position = self
            .events
            .partition_point(|event| event.sequence() < reference.sequence);
        Ok(self
            .events
            .get(position)
            .filter(|event| {
                event.sequence() == reference.sequence && event.event_id() == &reference.event_id
            })
            .cloned())
    }

    fn lab_related(&self, keys: &LabKeys, through: u64) -> GlobalLedgerResult<bool> {
        Ok(self.indexes.lab_related(keys, through))
    }
}

/// LN-5: the writer's history: the records committed to its store and the records its current
/// batch has staged, in ledger order.
pub(super) struct WriterHistory<'a, B> {
    backend: &'a B,
    staged: &'a [PersistedEvent],
    retention: &'a RetentionIndex,
    page_rows: usize,
}

impl<B: DurableStorage> WriterHistory<'_, B> {
    /// Every record `selection` selects, committed or staged, in ledger order.
    pub(super) fn select(
        &self,
        selection: &HistorySelection<'_>,
    ) -> GlobalLedgerResult<Vec<PersistedEvent>> {
        let head = self.backend.head();
        let through = selection.query.to_sequence.map_or(head, |to| to.min(head));
        let mut after = selection
            .query
            .from_sequence
            .map_or(0, |from| from.saturating_sub(1));
        let mut events = Vec::new();
        while after < through {
            let page = self.backend.history_page(
                selection,
                (after, through),
                self.page_rows,
                self.retention,
            )?;
            let full = page.len() >= self.page_rows;
            let Some(last) = page.last().map(PersistedEvent::sequence) else {
                break;
            };
            after = last;
            events.extend(page.into_iter().filter(|event| selection.selects(event)));
            if !full {
                break;
            }
        }
        events.extend(
            self.staged
                .iter()
                .filter(|event| selection.selects(*event))
                .cloned(),
        );
        Ok(events)
    }

    /// The record `reference` names, committed or staged.
    pub(super) fn record_at(
        &self,
        reference: &TerminalEvent,
    ) -> GlobalLedgerResult<Option<PersistedEvent>> {
        if let Some(event) = self
            .staged
            .iter()
            .find(|event| event.sequence() == reference.sequence)
        {
            return Ok((event.event_id() == &reference.event_id).then(|| event.clone()));
        }
        if reference.sequence == 0 || reference.sequence > self.backend.head() {
            return Ok(None);
        }
        let query = EventQuery::default();
        Ok(self
            .backend
            .history_page(
                &HistorySelection::of(&query),
                (reference.sequence - 1, reference.sequence),
                1,
                self.retention,
            )?
            .into_iter()
            .find(|event| {
                event.sequence() == reference.sequence && event.event_id() == &reference.event_id
            }))
    }

    /// Whether a committed or a staged record has `event_id` (the `event_id` unique index).
    pub(super) fn contains_event_id(&self, event_id: &EventId) -> GlobalLedgerResult<bool> {
        if self.staged.iter().any(|event| event.event_id() == event_id) {
            return Ok(true);
        }
        Ok(self.backend.committed_sequence(event_id)?.is_some())
    }

    /// The Lab relations of the committed and the staged records through `through`.
    pub(super) fn lab_relations(&self, through: u64) -> GlobalLedgerResult<LabRelations> {
        let mut staged = LabRelations::default();
        staged.add_anchors(self.staged, through);
        let mut relations = self.backend.lab_relations(through, &staged)?;
        relations.add_runs(self.staged, through);
        Ok(relations)
    }
}

impl<B: DurableStorage> RetentionHistory for WriterHistory<'_, B> {
    type Event = PersistedEvent;

    fn record(&self, reference: &TerminalEvent) -> GlobalLedgerResult<Option<PersistedEvent>> {
        self.record_at(reference)
    }

    fn lab_related(&self, keys: &LabKeys, through: u64) -> GlobalLedgerResult<bool> {
        Ok(self.lab_relations(through)?.related(keys))
    }
}

impl<B: DurableStorage> EventStore<B> {
    /// LN-5: the writer's history: its store and the batch the flush has staged.
    pub(super) fn history(&self) -> WriterHistory<'_, B> {
        WriterHistory {
            backend: &self.backend,
            staged: &self.staged,
            retention: &self.retention,
            page_rows: self.history_page_rows,
        }
    }

    /// LN-5 (§3.4): the records `query` selects, committed or staged, in ledger order (the
    /// lookups of the checks an append makes and of the scheduled-settlement continuation).
    pub(super) fn lookup(&self, query: &EventQuery) -> GlobalLedgerResult<Vec<PersistedEvent>> {
        self.history().select(&HistorySelection::of(query))
    }

    /// LN-5 (§3.3): the committed records `query` selects after `range.0` through `range.1`, in
    /// ledger order, at most `limit` of them, each with its eviction proofs; a Lab view relates a
    /// record as of `lab_through`. Every record the store returns counts on `work`; `None` when
    /// its budget stopped the read before the read ended (#411 A51).
    pub(super) fn read_history(
        &self,
        query: &EventQuery,
        (after, through): (u64, u64),
        limit: Option<usize>,
        lab_through: u64,
        work: &ReadWork,
    ) -> GlobalLedgerResult<Option<Vec<PersistedEvent>>> {
        let mut events = Vec::new();
        if limit == Some(0) {
            return Ok(Some(events));
        }
        // Only the Lab view reads the relation (#411 A51).
        let lab = match query.view {
            Some(LedgerView::Lab) => Some(
                self.backend
                    .lab_relations(lab_through, &LabRelations::default())?,
            ),
            _ => None,
        };
        let selection = HistorySelection::of(query);
        let mut cursor = after.max(query.from_sequence.map_or(0, |from| from.saturating_sub(1)));
        let through = through
            .min(query.to_sequence.unwrap_or(u64::MAX))
            .min(self.backend.head());
        while cursor < through {
            // #422 LN-TUNE: a read with a limit asks for no more rows than it can still
            // return, so no page holds a row past the one the limit is reached at; the next
            // read starts after that row, and each row is read and authenticated once.
            let page_rows = limit.map_or(self.history_page_rows, |limit| {
                limit
                    .saturating_sub(events.len())
                    .min(self.history_page_rows)
            });
            let page = self.backend.history_page(
                &selection,
                (cursor, through),
                page_rows,
                &self.retention,
            )?;
            #[cfg(test)]
            if let Some(counter) = &self.history_rows_read {
                counter.fetch_add(
                    u64::try_from(page.len()).unwrap_or(u64::MAX),
                    std::sync::atomic::Ordering::SeqCst,
                );
            }
            let full = page.len() >= page_rows;
            for mut event in page {
                cursor = event.sequence();
                let going_on = work.visit();
                let viewed = query.view.is_none_or(|view| {
                    view.contains(
                        event.event_type(),
                        event.severity(),
                        event.origin().source(),
                        view == LedgerView::Lab
                            && lab
                                .as_ref()
                                .is_some_and(|lab| lab.related(&LabKeys::of(event.links()))),
                    )
                });
                if selection.selects(&event) && viewed {
                    self.retention.annotate_event(&mut event);
                    events.push(event);
                    if limit == Some(events.len()) {
                        return Ok(Some(events));
                    }
                }
                if !going_on {
                    return Ok(None);
                }
            }
            if !full {
                break;
            }
        }
        Ok(Some(events))
    }
}
