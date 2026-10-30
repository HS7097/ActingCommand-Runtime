// SPDX-License-Identifier: AGPL-3.0-only

use super::GlobalLedgerError;
use crate::codes::{LedgerCode, LedgerLocation};
use crate::{PersistedEvent, fact::LedgerEventRead};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::{
    ActionId, AuthoritativeSchedulingOutcome, CausationId, CorrelationId, DiagnosticCode,
    EventLinks, EventPayload, EventQuery, EventSource, EventType, FrameId, InstanceId, LeaseId,
    LedgerView, OriginModule, PolicyPayload, ProjectedEvent, ProjectionPayload, ProjectionProfile,
    RecognitionId, RequestId, RunId, SchedulingOutcomeIdentity, SchedulingOutcomeProjection,
    TaskId, TaskOutcome, TaskPayload, TaskSemanticFact,
};
use std::collections::{BTreeMap, BTreeSet};

mod views;
pub(super) use views::{PageContext, page_bounds, project_selected};
// #422 LN-OFF-1: only the segment store's pages and the gate tests name a page selection; the
// offline snapshot without an SQLite root passes its head (`From<u64>`).
#[cfg(test)]
pub(super) use views::PageSelection;

/// The closed Lab link paths are shared by the in-memory and SQLite selectors.
#[derive(Clone, Copy)]
pub(super) enum LabAxis {
    Request,
    Correlation,
}

pub(super) const LAB_ANCHOR_SOURCE: EventSource = EventSource::Lab;
pub(super) const LAB_ANCHOR_TYPE: EventType = EventType::LabRequest;
pub(super) const LAB_RELATIONS: [(LabAxis, bool); 4] = [
    (LabAxis::Request, false),
    (LabAxis::Correlation, false),
    (LabAxis::Request, true),
    (LabAxis::Correlation, true),
];

/// Whether `event` is a Lab anchor: a Lab record or a Lab request.
pub(super) fn is_lab_anchor<E: LedgerEventRead>(event: &E) -> bool {
    event.origin().source() == LAB_ANCHOR_SOURCE || event.event_type() == LAB_ANCHOR_TYPE
}

/// LN-5 (#422, LEAN-L §3.3): the links a Lab relation reads of a record (`LAB_RELATIONS`): its
/// request, its correlation and its run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct LabKeys {
    pub(super) request: Option<RequestId>,
    pub(super) correlation: Option<CorrelationId>,
    pub(super) run: Option<RunId>,
}

impl LabKeys {
    pub(super) fn of(links: &EventLinks) -> Self {
        Self {
            request: links.request_id().copied(),
            correlation: links.correlation_id().copied(),
            run: links.run_id().copied(),
        }
    }

    /// #422 IDLE-OV (R2): whether the record names a link a Lab relation reads; a record that
    /// names none relates to no Lab anchor, whatever the relations are.
    pub(super) fn names_a_relation_link(&self) -> bool {
        self.request.is_some() || self.correlation.is_some() || self.run.is_some()
    }
}

#[derive(Default)]
pub(super) struct EventIndexes {
    event_types: BTreeMap<usize, BTreeSet<usize>>,
    origin_modules: BTreeMap<OriginModule, BTreeSet<usize>>,
    diagnostic_codes: BTreeMap<DiagnosticCode, BTreeSet<usize>>,
    instance_ids: BTreeMap<InstanceId, BTreeSet<usize>>,
    request_ids: BTreeMap<RequestId, BTreeSet<usize>>,
    correlation_ids: BTreeMap<CorrelationId, BTreeSet<usize>>,
    causation_ids: BTreeMap<CausationId, BTreeSet<usize>>,
    task_ids: BTreeMap<TaskId, BTreeSet<usize>>,
    run_ids: BTreeMap<RunId, BTreeSet<usize>>,
    lease_ids: BTreeMap<LeaseId, BTreeSet<usize>>,
    frame_ids: BTreeMap<FrameId, BTreeSet<usize>>,
    action_ids: BTreeMap<ActionId, BTreeSet<usize>>,
    recognition_ids: BTreeMap<RecognitionId, BTreeSet<usize>>,
    lab_requests: BTreeMap<RequestId, u64>,
    lab_correlations: BTreeMap<CorrelationId, u64>,
    run_requests: BTreeMap<RunId, BTreeMap<RequestId, u64>>,
    run_correlations: BTreeMap<RunId, BTreeMap<CorrelationId, u64>>,
    /// #411 A51 (R1): the runs each request and correlation appeared in.
    request_runs: BTreeMap<RequestId, BTreeSet<RunId>>,
    correlation_runs: BTreeMap<CorrelationId, BTreeSet<RunId>>,
    /// #411 A51 (R1): per run, the lowest position from which a Lab anchor relates to it
    /// through a request (or a correlation) the run shares with the anchor: the least, over
    /// the run's requests that some anchor names, of the later of the request's first position
    /// in the run and its first anchor. Kept at each insert, so one lookup answers what the
    /// stored view's correlated sub-select computes per row.
    run_request_lab: BTreeMap<RunId, u64>,
    run_correlation_lab: BTreeMap<RunId, u64>,
}

/// One budgeted scan of the candidates (#411 A51): the events it selected and, when its read's
/// budget stopped it, the sequence of the last candidate it decided.
pub(super) struct PageScan<E> {
    pub(super) events: Vec<E>,
    pub(super) stopped_at: Option<u64>,
}

/// Lowers `key`'s value in `map` to `value`.
fn lower<K: Ord>(map: &mut BTreeMap<K, u64>, key: K, value: u64) {
    map.entry(key)
        .and_modify(|current| *current = (*current).min(value))
        .or_insert(value);
}

impl EventIndexes {
    pub(super) fn from_events<E: LedgerEventRead>(events: &[E]) -> Self {
        let mut indexes = Self::default();
        for (position, event) in events.iter().enumerate() {
            indexes.insert(event, position);
        }
        indexes
    }

    pub(super) fn insert<E: LedgerEventRead>(&mut self, event: &E, position: usize) {
        self.event_types
            .entry(event_type_index(event.event_type()))
            .or_default()
            .insert(position);
        self.origin_modules
            .entry(event.origin().module())
            .or_default()
            .insert(position);
        insert_link(
            &mut self.diagnostic_codes,
            event.payload().diagnostic_code().as_ref(),
            position,
        );
        let links = event.links();
        insert_link(&mut self.instance_ids, links.instance_id(), position);
        insert_link(&mut self.request_ids, links.request_id(), position);
        insert_link(&mut self.correlation_ids, links.correlation_id(), position);
        insert_link(&mut self.causation_ids, links.causation_id(), position);
        insert_link(&mut self.task_ids, links.task_id(), position);
        insert_link(&mut self.run_ids, links.run_id(), position);
        insert_link(&mut self.lease_ids, links.lease_id(), position);
        insert_link(&mut self.frame_ids, links.frame_id(), position);
        insert_link(&mut self.action_ids, links.action_id(), position);
        insert_link(&mut self.recognition_ids, links.recognition_id(), position);
        let sequence = event.sequence();
        if is_lab_anchor(event) {
            if let Some(request) = links.request_id()
                && !self.lab_requests.contains_key(request)
            {
                self.lab_requests.insert(*request, sequence);
                // #411 A51 (R1): each run the request appeared in relates to Lab from the later
                // of the request's first position in that run and this, its first anchor.
                for run in self.request_runs.get(request).into_iter().flatten() {
                    if let Some(first) = self
                        .run_requests
                        .get(run)
                        .and_then(|requests| requests.get(request))
                    {
                        lower(&mut self.run_request_lab, *run, (*first).max(sequence));
                    }
                }
            }
            if let Some(correlation) = links.correlation_id()
                && !self.lab_correlations.contains_key(correlation)
            {
                self.lab_correlations.insert(*correlation, sequence);
                for run in self.correlation_runs.get(correlation).into_iter().flatten() {
                    if let Some(first) = self
                        .run_correlations
                        .get(run)
                        .and_then(|correlations| correlations.get(correlation))
                    {
                        lower(&mut self.run_correlation_lab, *run, (*first).max(sequence));
                    }
                }
            }
        }
        if let Some(run) = links.run_id() {
            if let Some(request) = links.request_id() {
                let requests = self.run_requests.entry(*run).or_default();
                if !requests.contains_key(request) {
                    requests.insert(*request, sequence);
                    self.request_runs.entry(*request).or_default().insert(*run);
                    // #411 A51 (R1): the request's first position in this run, once an anchor
                    // names the request.
                    if let Some(anchor) = self.lab_requests.get(request) {
                        lower(&mut self.run_request_lab, *run, (*anchor).max(sequence));
                    }
                }
            }
            if let Some(correlation) = links.correlation_id() {
                let correlations = self.run_correlations.entry(*run).or_default();
                if !correlations.contains_key(correlation) {
                    correlations.insert(*correlation, sequence);
                    self.correlation_runs
                        .entry(*correlation)
                        .or_default()
                        .insert(*run);
                    if let Some(anchor) = self.lab_correlations.get(correlation) {
                        lower(&mut self.run_correlation_lab, *run, (*anchor).max(sequence));
                    }
                }
            }
        }
    }

    pub(super) fn query<E: LedgerEventRead>(&self, events: &[E], query: &EventQuery) -> Vec<E> {
        self.query_with_observer(events, query, &mut || true).events
    }

    /// Every candidate is decided before `observe_candidate` may stop the scan (#411 A51).
    fn query_with_observer<E: LedgerEventRead>(
        &self,
        events: &[E],
        query: &EventQuery,
        observe_candidate: &mut impl FnMut() -> bool,
    ) -> PageScan<E> {
        let minimum_sequence = query.from_sequence.unwrap_or(0);
        let start = events.partition_point(|event| event.sequence() < minimum_sequence);
        let snapshot = events.last().map_or(0, E::sequence);
        let mut scan = PageScan {
            events: Vec::new(),
            stopped_at: None,
        };
        for event in self.candidates_from(events, query, start) {
            let going_on = observe_candidate();
            if self.matches(query, event, snapshot) {
                scan.events.push(event.clone());
            }
            if !going_on {
                scan.stopped_at = Some(event.sequence());
                break;
            }
        }
        scan
    }

    pub(super) fn query_page<E: LedgerEventRead>(
        &self,
        events: &[E],
        query: &EventQuery,
        after_sequence: u64,
        through_sequence: u64,
        page_events: usize,
    ) -> Vec<E> {
        self.query_page_with_observer(
            events,
            query,
            after_sequence,
            through_sequence,
            page_events,
            &mut || true,
        )
        .events
    }

    /// The candidates in sequence order up to `through_sequence`, until `page_events` match.
    /// `observe_candidate` sees each candidate examined; every candidate is decided before it
    /// may stop the scan, which then names that candidate (#411 A51: every position up to it
    /// is decided).
    fn query_page_with_observer<E: LedgerEventRead>(
        &self,
        events: &[E],
        query: &EventQuery,
        after_sequence: u64,
        through_sequence: u64,
        page_events: usize,
        observe_candidate: &mut impl FnMut() -> bool,
    ) -> PageScan<E> {
        let minimum_sequence = query
            .from_sequence
            .unwrap_or(0)
            .max(after_sequence.saturating_add(1));
        let start = events.partition_point(|event| event.sequence() < minimum_sequence);
        let mut scan = PageScan {
            events: Vec::new(),
            stopped_at: None,
        };
        if page_events == 0 {
            return scan;
        }
        for event in self.candidates_from(events, query, start) {
            if event.sequence() > through_sequence {
                break;
            }
            let going_on = observe_candidate();
            if event.sequence() > after_sequence && self.matches(query, event, through_sequence) {
                scan.events.push(event.clone());
                if scan.events.len() == page_events {
                    break;
                }
            }
            if !going_on {
                scan.stopped_at = Some(event.sequence());
                break;
            }
        }
        scan
    }

    #[cfg(test)]
    pub(super) fn query_page_with_visit_count<E: LedgerEventRead>(
        &self,
        events: &[E],
        query: &EventQuery,
        after_sequence: u64,
        through_sequence: u64,
        page_events: usize,
    ) -> (Vec<E>, usize) {
        let mut visited = 0;
        let page = self.query_page_with_observer(
            events,
            query,
            after_sequence,
            through_sequence,
            page_events,
            &mut || {
                visited += 1;
                true
            },
        );
        (page.events, visited)
    }

    fn candidates_from<'a, E: LedgerEventRead>(
        &'a self,
        events: &'a [E],
        query: &EventQuery,
        start: usize,
    ) -> Box<dyn Iterator<Item = &'a E> + 'a> {
        let event_type = query.event_type.map(event_type_index);
        let candidates = [
            indexed_filter(&self.event_types, event_type.as_ref()),
            indexed_filter(&self.origin_modules, query.origin_module.as_ref()),
            indexed_filter(&self.diagnostic_codes, query.diagnostic_code.as_ref()),
            indexed_filter(&self.instance_ids, query.instance_id.as_ref()),
            indexed_filter(&self.request_ids, query.request_id.as_ref()),
            indexed_filter(&self.correlation_ids, query.correlation_id.as_ref()),
            indexed_filter(&self.causation_ids, query.causation_id.as_ref()),
            indexed_filter(&self.task_ids, query.task_id.as_ref()),
            indexed_filter(&self.run_ids, query.run_id.as_ref()),
            indexed_filter(&self.lease_ids, query.lease_id.as_ref()),
            indexed_filter(&self.frame_ids, query.frame_id.as_ref()),
            indexed_filter(&self.action_ids, query.action_id.as_ref()),
            indexed_filter(&self.recognition_ids, query.recognition_id.as_ref()),
        ];
        if candidates
            .iter()
            .any(|candidate| matches!(candidate, Some(None)))
        {
            return Box::new(std::iter::empty());
        }
        // The instance set is the union of its index positions (OR within the set).
        let instance_union: Option<BTreeSet<usize>> = (!query.instance_ids.is_empty()).then(|| {
            query
                .instance_ids
                .iter()
                .filter_map(|instance_id| self.instance_ids.get(instance_id))
                .flat_map(|positions| positions.range(start..).copied())
                .collect()
        });
        if instance_union.as_ref().is_some_and(BTreeSet::is_empty) {
            return Box::new(std::iter::empty());
        }
        // Query predicates still validate every result; borrowing the smallest index avoids
        // materializing full-set intersections while preserving all filter combinations.
        let borrowed = |positions: &'a BTreeSet<usize>| -> Box<dyn Iterator<Item = &'a E> + 'a> {
            Box::new(
                positions
                    .range(start..)
                    .map(move |position| &events[*position]),
            )
        };
        let smallest = candidates
            .into_iter()
            .filter_map(Option::flatten)
            .min_by_key(|positions| positions.len());
        match (smallest, instance_union) {
            (Some(positions), None) => borrowed(positions),
            (Some(positions), Some(union)) if positions.len() <= union.len() => borrowed(positions),
            (_, Some(union)) => Box::new(union.into_iter().map(move |position| &events[position])),
            (None, None) => Box::new(events[start..].iter()),
        }
    }

    fn matches<E: LedgerEventRead>(&self, query: &EventQuery, event: &E, snapshot: u64) -> bool {
        query_matches_fields(query, event)
            && query.view.is_none_or(|view| {
                view.contains(
                    event.event_type(),
                    event.severity(),
                    event.origin().source(),
                    // Only the Lab view reads the relation (#411 A51).
                    view == LedgerView::Lab
                        && self.lab_related(&LabKeys::of(event.links()), snapshot),
                )
            })
    }

    /// Whether a Lab anchor at or before `snapshot` relates to a record with `keys` through
    /// `LAB_RELATIONS`. #411 A51 (R1): each relation is one lookup of a per-link-group fact kept
    /// at insert (the first anchor of a request or a correlation, and per run the position from
    /// which one of its requests or correlations has an anchor), never a walk over the anchors or
    /// a run's links.
    pub(super) fn lab_related(&self, keys: &LabKeys, snapshot: u64) -> bool {
        let within =
            |sequence: Option<&u64>| sequence.is_some_and(|sequence| *sequence <= snapshot);
        LAB_RELATIONS
            .into_iter()
            .any(|(axis, via_run)| match (axis, via_run) {
                (LabAxis::Request, false) => keys
                    .request
                    .is_some_and(|request| within(self.lab_requests.get(&request))),
                (LabAxis::Correlation, false) => keys
                    .correlation
                    .is_some_and(|correlation| within(self.lab_correlations.get(&correlation))),
                (LabAxis::Request, true) => keys
                    .run
                    .is_some_and(|run| within(self.run_request_lab.get(&run))),
                (LabAxis::Correlation, true) => keys
                    .run
                    .is_some_and(|run| within(self.run_correlation_lab.get(&run))),
            })
    }
}

pub(super) fn project<E: LedgerEventRead>(event: &E, profile: ProjectionProfile) -> ProjectedEvent {
    project_at(event, profile, u64::MAX)
}

pub(super) fn project_at<E: LedgerEventRead>(
    event: &E,
    profile: ProjectionProfile,
    snapshot: u64,
) -> ProjectedEvent {
    let (payload, include_object_key) = match profile {
        ProjectionProfile::Cli | ProjectionProfile::Concise => (ProjectionPayload::Omitted, false),
        ProjectionProfile::Lab | ProjectionProfile::Verbose
            if matches!(
                event.event_type(),
                EventType::RuntimeLifecycleObserved | EventType::ProviderStartupObserved
            ) =>
        {
            (
                ProjectionPayload::Public(Box::new(event.payload().public_projection())),
                false,
            )
        }
        ProjectionProfile::Ui | ProjectionProfile::Normal => (
            ProjectionPayload::Public(Box::new(event.payload().public_projection())),
            false,
        ),
        ProjectionProfile::Lab | ProjectionProfile::Verbose | ProjectionProfile::Forensic => (
            ProjectionPayload::Full(Box::new(event.payload().clone())),
            true,
        ),
    };
    ProjectedEvent {
        schema_version: event.schema_version().to_string(),
        sequence: event.sequence(),
        event_id: *event.event_id(),
        timestamp_unix_ms: event.timestamp_unix_ms(),
        event_type: event.event_type(),
        severity: event.severity(),
        sensitivity: event.sensitivity(),
        origin: event.origin().clone(),
        links: event.links().clone(),
        payload_schema: event.payload_schema().to_string(),
        payload,
        artifacts: event.projected_artifacts(include_object_key),
        artifact_evictions: event
            .artifact_evictions()
            .iter()
            .filter_map(|proof| proof.observation(snapshot))
            .collect(),
        // Snapshot-aware page projection fills the complete overlapping membership.
        views: Vec::new(),
    }
}

pub(super) fn project_if_matches(
    event: &PersistedEvent,
    query: &EventQuery,
    profile: ProjectionProfile,
) -> Option<ProjectedEvent> {
    query_matches(query, event).then(|| project(event, profile))
}

pub(super) fn project_scheduling_outcomes(
    terminals: &[PersistedEvent],
    admissions: &[PersistedEvent],
    leases: &[PersistedEvent],
    ledger_position: u64,
    expected: &SchedulingOutcomeIdentity,
) -> Result<SchedulingOutcomeProjection, GlobalLedgerError> {
    if ledger_position < expected.terminal_sequence() {
        return Err(GlobalLedgerError::request(
            SharedCode::OutcomeProjectionNotReady.code(),
            LedgerLocation::ProjectSchedulingOutcomes.location(),
        ));
    }
    let [terminal] = terminals else {
        return Err(GlobalLedgerError::fatal(
            LedgerCode::OutcomeProjectionTerminalNotUnique.code(),
            LedgerLocation::ProjectSchedulingOutcomes.location(),
        ));
    };
    let [admitted] = admissions else {
        return Err(GlobalLedgerError::fatal(
            LedgerCode::OutcomeProjectionAdmissionNotUnique.code(),
            LedgerLocation::ProjectSchedulingOutcomes.location(),
        ));
    };
    let [lease_granted] = leases else {
        return Err(GlobalLedgerError::fatal(
            LedgerCode::OutcomeProjectionLeaseNotUnique.code(),
            LedgerLocation::ProjectSchedulingOutcomes.location(),
        ));
    };
    let EventPayload::Task(TaskPayload::Semantic(terminal_payload)) = terminal.payload() else {
        return Err(GlobalLedgerError::fatal(
            LedgerCode::OutcomeProjectionTerminalInvalid.code(),
            LedgerLocation::ProjectSchedulingOutcomes.location(),
        ));
    };
    let TaskSemanticFact::TerminalCommitted {
        task_result: TaskOutcome::Success,
        outcome: None,
        scheduling_disposition: Some(disposition),
        ..
    } = terminal_payload.fact()
    else {
        return Err(GlobalLedgerError::fatal(
            LedgerCode::OutcomeProjectionTerminalInvalid.code(),
            LedgerLocation::ProjectSchedulingOutcomes.location(),
        ));
    };
    let EventPayload::Policy(PolicyPayload::DispatchAdmitted(admission)) = admitted.payload()
    else {
        return Err(GlobalLedgerError::fatal(
            LedgerCode::OutcomeProjectionAdmissionInvalid.code(),
            LedgerLocation::ProjectSchedulingOutcomes.location(),
        ));
    };
    let terminal_links = terminal.links();
    let admission_links = admitted.links();
    let lease_links = lease_granted.links();
    if terminal.event_id() != &expected.terminal_event_id()
        || terminal.sequence() != expected.terminal_sequence()
        || terminal_links.instance_id() != Some(&expected.instance_id())
        || terminal_links.request_id() != Some(&expected.request_id())
        || terminal_links.correlation_id() != Some(&expected.correlation_id())
        || terminal_links.task_id() != Some(&expected.task_id())
        || terminal_links.run_id() != Some(&expected.run_id())
        || terminal_links.lease_id() != Some(&expected.lease_id())
        || admission_links.instance_id() != Some(&expected.instance_id())
        || admission_links.correlation_id() != Some(&expected.correlation_id())
        || admission_links.task_id() != Some(&expected.task_id())
        || admission_links.run_id() != Some(&expected.run_id())
        || lease_links.instance_id() != Some(&expected.instance_id())
        || lease_links.correlation_id() != Some(&expected.correlation_id())
        || lease_links.task_id() != Some(&expected.task_id())
        || lease_links.run_id() != Some(&expected.run_id())
        || lease_links.lease_id() != Some(&expected.lease_id())
        || admission.decision_id() != expected.decision_id()
        || admission.task_id() != expected.catalog_task_id()
        || admission.instance_id() != expected.instance_alias()
        || lease_granted.sequence() >= admitted.sequence()
        || admitted.sequence() >= terminal.sequence()
    {
        return Err(GlobalLedgerError::fatal(
            LedgerCode::OutcomeProjectionIdentityMismatch.code(),
            LedgerLocation::ProjectSchedulingOutcomes.location(),
        ));
    }
    let outcome = AuthoritativeSchedulingOutcome::new(
        expected.clone(),
        disposition.clone(),
        terminal.timestamp_unix_ms(),
    )
    .map_err(|_| {
        GlobalLedgerError::fatal(
            LedgerCode::OutcomeProjectionIdentityInvalid.code(),
            LedgerLocation::ProjectSchedulingOutcomes.location(),
        )
    })?;
    SchedulingOutcomeProjection::new(ledger_position, outcome).map_err(|_| {
        GlobalLedgerError::fatal(
            LedgerCode::OutcomeProjectionInvalid.code(),
            LedgerLocation::ProjectSchedulingOutcomes.location(),
        )
    })
}

pub(crate) fn query_matches(query: &EventQuery, event: &PersistedEvent) -> bool {
    query_matches_fields(query, event)
        && query.view.is_none_or(|view| {
            view.contains(
                event.event_type(),
                event.severity(),
                event.origin().source(),
                false,
            )
        })
}

/// The query's field conditions on one event (Workflow #411 A4: also the range read's match).
pub(super) fn query_matches_fields<E: LedgerEventRead>(query: &EventQuery, event: &E) -> bool {
    let links = event.links();
    query
        .from_sequence
        .is_none_or(|value| event.sequence() >= value)
        && query
            .to_sequence
            .is_none_or(|value| event.sequence() <= value)
        && query
            .event_type
            .is_none_or(|value| event.event_type() == value)
        // Workflow #389 MCP-2: OR within the selected set, AND with the rest.
        && (query.event_types.is_empty() || query.event_types.contains(&event.event_type()))
        && !query.exclude_event_types.contains(&event.event_type())
        && query
            .fact_key
            .as_deref()
            .is_none_or(|key| event.payload().names_fact_key(key))
        && query
            .minimum_severity
            .is_none_or(|value| event.severity() >= value)
        && query
            .maximum_severity
            .is_none_or(|value| event.severity() <= value)
        && query
            .from_timestamp_unix_ms
            .is_none_or(|value| event.timestamp_unix_ms() >= value)
        && query
            .to_timestamp_unix_ms
            .is_none_or(|value| event.timestamp_unix_ms() < value)
        && query
            .source
            .is_none_or(|value| event.origin().source() == value)
        && query
            .origin_module
            .is_none_or(|value| event.origin().module() == value)
        && query
            .diagnostic_code
            .is_none_or(|value| event.payload().diagnostic_code() == Some(value))
        && link_matches(query.instance_id.as_ref(), links.instance_id())
        && (query.instance_ids.is_empty()
            || links
                .instance_id()
                .is_some_and(|instance_id| query.instance_ids.contains(instance_id)))
        && link_matches(query.request_id.as_ref(), links.request_id())
        && link_matches(query.correlation_id.as_ref(), links.correlation_id())
        && link_matches(query.causation_id.as_ref(), links.causation_id())
        && link_matches(query.task_id.as_ref(), links.task_id())
        && link_matches(query.run_id.as_ref(), links.run_id())
        && link_matches(query.lease_id.as_ref(), links.lease_id())
        && link_matches(query.frame_id.as_ref(), links.frame_id())
        && link_matches(query.action_id.as_ref(), links.action_id())
        && link_matches(query.recognition_id.as_ref(), links.recognition_id())
}

fn insert_link<K: Clone + Ord>(
    index: &mut BTreeMap<K, BTreeSet<usize>>,
    value: Option<&K>,
    position: usize,
) {
    if let Some(value) = value {
        index.entry(value.clone()).or_default().insert(position);
    }
}

fn indexed_filter<'a, K: Ord>(
    index: &'a BTreeMap<K, BTreeSet<usize>>,
    value: Option<&K>,
) -> Option<Option<&'a BTreeSet<usize>>> {
    value.map(|value| index.get(value))
}

fn event_type_index(event_type: EventType) -> usize {
    event_type as usize
}

fn link_matches<K: PartialEq>(expected: Option<&K>, actual: Option<&K>) -> bool {
    expected.is_none_or(|expected| actual == Some(expected))
}
