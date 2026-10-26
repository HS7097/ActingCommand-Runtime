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
    ArtifactId, ArtifactKind, CorrelationId, EventId, EventPayload, EventQuery, EventType, FrameId,
    InstanceId, LeaseId, LedgerView, PolicyPayload, RequestId, RunId, TerminalEvent,
};
use std::collections::BTreeSet;
use std::time::Instant;

/// The default of `ledger_observer.history_page_rows`: the rows of one page of a history read.
pub(super) const DEFAULT_HISTORY_PAGE_ROWS: usize = 256;
/// The largest `ledger_observer.history_page_rows`.
pub(super) const MAX_HISTORY_PAGE_ROWS: usize = 4096;
/// #422 LN-WS (R2): the default of `ledger_observer.history_scan_rows`, the sequences one history
/// statement of a view page scans at most.
pub(super) const DEFAULT_HISTORY_SCAN_ROWS: usize = 65_536;
/// #422 LN-WS: the largest `ledger_observer.history_scan_rows`.
pub(super) const MAX_HISTORY_SCAN_ROWS: usize = 1_048_576;

/// What a history read selects: the records `query` names and, when `decision_id` is set, only
/// the policy dispatch and execution records of that decision. The store selects by its indexes
/// (the sequence key; the event type, origin, time and severity indexes; the link indexes; a
/// decision through the record's payload); the reader checks every selected record against the
/// whole selection again (`selects`), and against the query's view.
#[derive(Clone, Copy)]
pub(super) struct HistorySelection<'a> {
    pub(super) query: &'a EventQuery,
    pub(super) decision_id: Option<&'a str>,
    /// LN-7 (#422, LEAN-L §3.5): when set, only the records linked to one of its identifiers.
    pub(super) links: Option<&'a LinkSet>,
}

impl<'a> HistorySelection<'a> {
    pub(super) const fn of(query: &'a EventQuery) -> Self {
        Self {
            query,
            decision_id: None,
            links: None,
        }
    }

    /// LN-7: the records `query` selects that are linked to one of `links`' identifiers.
    pub(super) const fn linked(query: &'a EventQuery, links: &'a LinkSet) -> Self {
        Self {
            query,
            decision_id: None,
            links: Some(links),
        }
    }

    /// Whether `event` meets every condition of the selection, the query's view aside.
    pub(super) fn selects<E: LedgerEventRead>(&self, event: &E) -> bool {
        query_matches_fields(self.query, event)
            && self
                .decision_id
                .is_none_or(|decision| decision_of(event) == Some(decision))
            && self.links.is_none_or(|links| links.matches(event))
    }
}

/// LN-7 (#422, LEAN-L §3.5): the identifiers a retention read selects records by: a record is
/// selected when its run, lease, frame, request or correlation link is one of them, or when one
/// of the artifacts it carries is (`ledger_artifacts`, by its `artifact_id` index).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct LinkSet {
    pub(super) runs: BTreeSet<RunId>,
    pub(super) leases: BTreeSet<LeaseId>,
    pub(super) frames: BTreeSet<FrameId>,
    pub(super) requests: BTreeSet<RequestId>,
    pub(super) correlations: BTreeSet<CorrelationId>,
    pub(super) artifacts: BTreeSet<ArtifactId>,
}

impl LinkSet {
    /// Whether `event` is linked to one of these identifiers.
    pub(super) fn matches<E: LedgerEventRead>(&self, event: &E) -> bool {
        let links = event.links();
        links.run_id().is_some_and(|run| self.runs.contains(run))
            || links
                .lease_id()
                .is_some_and(|lease| self.leases.contains(lease))
            || links
                .frame_id()
                .is_some_and(|frame| self.frames.contains(frame))
            || links
                .request_id()
                .is_some_and(|request| self.requests.contains(request))
            || links
                .correlation_id()
                .is_some_and(|correlation| self.correlations.contains(correlation))
            || !self.artifacts.is_empty()
                && event
                    .projected_artifacts(false)
                    .iter()
                    .any(|artifact| self.artifacts.contains(&artifact.artifact_id))
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

/// LN-7 (#422, LEAN-L §3.5): one closure scope of a range of records: its instance and its run
/// and lease links, or without a run its request and correlation links, with its first record and
/// the sequence of its last. A retention fact names no scope source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ScopeExtent {
    pub(super) instance: InstanceId,
    pub(super) run: Option<RunId>,
    pub(super) lease: Option<LeaseId>,
    pub(super) request: Option<RequestId>,
    pub(super) correlation: Option<CorrelationId>,
    pub(super) first: TerminalEvent,
    pub(super) last: u64,
}

/// LN-7: the scope extents of `events` (in ledger order), as a replay groups them (the test
/// medium's grouping; SQLite groups through its link index).
#[cfg(test)]
pub(super) fn scope_extents<'e, E: LedgerEventRead + 'e>(
    events: impl IntoIterator<Item = &'e E>,
) -> Vec<ScopeExtent> {
    let mut extents = std::collections::BTreeMap::new();
    for event in events {
        if event.payload().artifact_retention().is_some() {
            continue;
        }
        let links = event.links();
        let Some(instance) = links.instance_id().copied() else {
            continue;
        };
        let run = links.run_id().copied();
        let (request, correlation) = match run {
            Some(_) => (None, None),
            None => match (links.request_id(), links.correlation_id()) {
                (Some(request), Some(correlation)) => (Some(*request), Some(*correlation)),
                _ => continue,
            },
        };
        let key = (
            instance,
            run,
            links.lease_id().copied(),
            request,
            correlation,
        );
        extents
            .entry(key)
            .and_modify(|value: &mut (TerminalEvent, u64)| value.1 = event.sequence())
            .or_insert((
                TerminalEvent {
                    event_id: *event.event_id(),
                    sequence: event.sequence(),
                },
                event.sequence(),
            ));
    }
    extents
        .into_iter()
        .map(
            |((instance, run, lease, request, correlation), (first, last))| ScopeExtent {
                instance,
                run,
                lease,
                request,
                correlation,
                first,
                last,
            },
        )
        .collect()
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

/// LN-8d (#422): fails `ledger_read_budget_exceeded` when `deadline` is reached.
pub(super) fn check_deadline(deadline: Option<Instant>) -> GlobalLedgerResult<()> {
    match deadline {
        Some(deadline) if Instant::now() >= deadline => Err(super::GlobalLedgerError::request(
            crate::codes::LedgerCode::LedgerReadBudgetExceeded.code(),
            crate::codes::LedgerLocation::OpenRuntimeLedger.location(),
        )
        .with_detail("deadline reached at the open's retention read".to_owned())),
        _ => Ok(()),
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
    /// LN-8d (#422): the open's deadline, checked before each page a read of the open reads.
    deadline: Option<Instant>,
}

impl<B: DurableStorage> WriterHistory<'_, B> {
    /// Every record `selection` selects, committed or staged, in ledger order.
    pub(super) fn select(
        &self,
        selection: &HistorySelection<'_>,
    ) -> GlobalLedgerResult<Vec<PersistedEvent>> {
        self.select_where(selection, |_| true)
    }

    /// LN-7 (#422): the records `selection` selects that `keep` keeps, committed or staged, in
    /// ledger order; the others are held only while their page is read.
    pub(super) fn select_where(
        &self,
        selection: &HistorySelection<'_>,
        keep: impl Fn(&PersistedEvent) -> bool,
    ) -> GlobalLedgerResult<Vec<PersistedEvent>> {
        let head = self.backend.head();
        let through = selection.query.to_sequence.map_or(head, |to| to.min(head));
        let mut after = selection
            .query
            .from_sequence
            .map_or(0, |from| from.saturating_sub(1));
        let mut events = Vec::new();
        while after < through {
            check_deadline(self.deadline)?;
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
            events.extend(
                page.into_iter()
                    .filter(|event| selection.selects(event) && keep(event)),
            );
            if !full {
                break;
            }
        }
        events.extend(
            self.staged
                .iter()
                .filter(|event| selection.selects(*event) && keep(event))
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
        self.history_until(None)
    }

    /// LN-8d (#422): the writer's history read before `deadline`: a read that has a page left to
    /// read when the deadline is reached fails `ledger_read_budget_exceeded`, as the open's other
    /// phases do (`ledger_observer.open_timeout_ms`).
    pub(super) fn history_until(&self, deadline: Option<Instant>) -> WriterHistory<'_, B> {
        WriterHistory {
            backend: &self.backend,
            staged: &self.staged,
            retention: &self.retention,
            page_rows: self.history_page_rows,
            deadline,
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
            // #422 LN-WS (R1): a request's page stops where its statement bound interrupted it.
            let Some(page) = self.backend.history_page_within(
                &selection,
                (cursor, through),
                page_rows,
                &self.retention,
                work,
            )?
            else {
                return Ok(None);
            };
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

    /// #422 LN-7c C (`rd-1026/ln7c/final-query.md` §4): whether the store holds the capture frame
    /// an evidence read confirmation names: the frame's own `artifact.verified` record, selected
    /// by the `artifact_id` index, or for a run one such record among the run's, selected by its
    /// link index. Pages of `history_page_rows` records, each authenticated; the first match
    /// ends the read. `None` when `work`'s budget stopped it first.
    pub(super) fn evidence_target_known(
        &self,
        target: &LedgerEvidenceTarget,
        work: &ReadWork,
    ) -> GlobalLedgerResult<Option<bool>> {
        let links = match target {
            LedgerEvidenceTarget::Frame(artifact) => LinkSet {
                artifacts: BTreeSet::from([*artifact]),
                ..LinkSet::default()
            },
            LedgerEvidenceTarget::Run(run) => LinkSet {
                runs: BTreeSet::from([*run]),
                ..LinkSet::default()
            },
        };
        let query = EventQuery::default();
        let selection = HistorySelection::linked(&query, &links);
        let through = self.backend.head();
        let mut cursor = 0;
        while cursor < through {
            let Some(page) = self.backend.history_page_within(
                &selection,
                (cursor, through),
                self.history_page_rows,
                &self.retention,
                work,
            )?
            else {
                return Ok(None);
            };
            let full = page.len() >= self.history_page_rows;
            for event in &page {
                cursor = event.sequence();
                let going_on = work.visit();
                if selection.selects(event) && target.is_verified_frame(event) {
                    return Ok(Some(true));
                }
                if !going_on {
                    return Ok(None);
                }
            }
            if !full {
                break;
            }
        }
        Ok(Some(false))
    }

    /// #419 LN-WS-F (R2): the committed records after `range.0` through `range.1` that carry one
    /// of `artifacts`, in ledger order, each with its eviction proofs. They are selected by the
    /// `artifact_id` index of `ledger_artifacts` (a link set of artifacts, LN-8d's page form), a
    /// page of `history_page_rows` records at a time, so the read follows those artifacts'
    /// records, never the history of their event types. Every record the store returns counts on
    /// `work`; `None` when its budget stopped the read before the read ended. No artifact selects
    /// no record.
    pub(super) fn read_artifact_records(
        &self,
        artifacts: &BTreeSet<ArtifactId>,
        (after, through): (u64, u64),
        work: &ReadWork,
    ) -> GlobalLedgerResult<Option<Vec<PersistedEvent>>> {
        let mut events = Vec::new();
        if artifacts.is_empty() {
            return Ok(Some(events));
        }
        let links = LinkSet {
            artifacts: artifacts.clone(),
            ..LinkSet::default()
        };
        let query = EventQuery::default();
        let selection = HistorySelection::linked(&query, &links);
        let through = through.min(self.backend.head());
        let mut cursor = after;
        while cursor < through {
            let Some(page) = self.backend.history_page_within(
                &selection,
                (cursor, through),
                self.history_page_rows,
                &self.retention,
                work,
            )?
            else {
                return Ok(None);
            };
            let full = page.len() >= self.history_page_rows;
            for mut event in page {
                cursor = event.sequence();
                let going_on = work.visit();
                if selection.selects(&event) {
                    self.retention.annotate_event(&mut event);
                    events.push(event);
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

/// #422 LN-7c C (`rd-1026/ln7c/final-query.md` §4): what an evidence read confirmation names: one
/// capture frame by its artifact id, or a run (every frame captured in it before the
/// confirmation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedgerEvidenceTarget {
    Frame(ArtifactId),
    Run(RunId),
}

impl LedgerEvidenceTarget {
    /// Whether `event` is the `artifact.verified` record of a capture frame this target names.
    fn is_verified_frame(&self, event: &PersistedEvent) -> bool {
        event.event_type() == EventType::ArtifactVerified
            && event.projected_artifacts(false).iter().any(|reference| {
                reference.kind == ArtifactKind::CaptureFrame
                    && match self {
                        Self::Frame(artifact) => reference.artifact_id == *artifact,
                        Self::Run(run) => event.links().run_id() == Some(run),
                    }
            })
    }
}
