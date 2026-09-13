// SPDX-License-Identifier: AGPL-3.0-only

use super::GlobalLedgerError;
use crate::{PersistedEvent, fact::LedgerEventRead};
use actingcommand_contract::{
    ActionId, AuthoritativeSchedulingOutcome, CausationId, CorrelationId, DiagnosticCode, EventId,
    EventPayload, EventQuery, EventSource, EventType, FrameId, InstanceId, LeaseId, LedgerView,
    OriginModule, PolicyPayload, ProjectedEvent, ProjectionPayload, ProjectionProfile,
    RecognitionId, RequestId, RunId, SchedulingOutcomeIdentity, SchedulingOutcomeProjection,
    TaskId, TaskOutcome, TaskPayload, TaskSemanticFact,
};
use std::collections::{BTreeMap, BTreeSet};

mod views;
pub(super) use views::{PageSelection, page_bounds};

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

#[derive(Default)]
pub(super) struct EventIndexes {
    event_ids: BTreeMap<EventId, usize>,
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
}

impl EventIndexes {
    pub(super) fn from_events<E: LedgerEventRead>(events: &[E]) -> Self {
        let mut indexes = Self::default();
        for (position, event) in events.iter().enumerate() {
            indexes.insert(event, position);
        }
        indexes
    }

    pub(super) fn contains_event_id(&self, event_id: &EventId) -> bool {
        self.event_ids.contains_key(event_id)
    }

    pub(super) fn insert<E: LedgerEventRead>(&mut self, event: &E, position: usize) {
        self.event_ids.insert(*event.event_id(), position);
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
        if event.origin().source() == LAB_ANCHOR_SOURCE || event.event_type() == LAB_ANCHOR_TYPE {
            if let Some(request) = links.request_id() {
                self.lab_requests.entry(*request).or_insert(sequence);
            }
            if let Some(correlation) = links.correlation_id() {
                self.lab_correlations
                    .entry(*correlation)
                    .or_insert(sequence);
            }
        }
        if let Some(run) = links.run_id() {
            if let Some(request) = links.request_id() {
                self.run_requests
                    .entry(*run)
                    .or_default()
                    .entry(*request)
                    .or_insert(sequence);
            }
            if let Some(correlation) = links.correlation_id() {
                self.run_correlations
                    .entry(*run)
                    .or_default()
                    .entry(*correlation)
                    .or_insert(sequence);
            }
        }
    }

    pub(super) fn query<E: LedgerEventRead>(&self, events: &[E], query: &EventQuery) -> Vec<E> {
        let minimum_sequence = query.from_sequence.unwrap_or(0);
        let start = events.partition_point(|event| event.sequence() < minimum_sequence);
        let snapshot = events.last().map_or(0, E::sequence);
        self.candidates_from(events, query, start)
            .filter(|event| self.matches(query, *event, snapshot))
            .cloned()
            .collect()
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
            &mut || {},
        )
    }

    fn query_page_with_observer<E: LedgerEventRead>(
        &self,
        events: &[E],
        query: &EventQuery,
        after_sequence: u64,
        through_sequence: u64,
        page_events: usize,
        observe_candidate: &mut impl FnMut(),
    ) -> Vec<E> {
        let minimum_sequence = query
            .from_sequence
            .unwrap_or(0)
            .max(after_sequence.saturating_add(1));
        let start = events.partition_point(|event| event.sequence() < minimum_sequence);
        self.candidates_from(events, query, start)
            .take_while(|event| event.sequence() <= through_sequence)
            .filter(|event| {
                observe_candidate();
                event.sequence() > after_sequence && self.matches(query, *event, through_sequence)
            })
            .take(page_events)
            .cloned()
            .collect()
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
            &mut || visited += 1,
        );
        (page, visited)
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
        // Query predicates still validate every result; borrowing the smallest index avoids
        // materializing full-set intersections while preserving all filter combinations.
        match candidates
            .into_iter()
            .filter_map(Option::flatten)
            .min_by_key(|positions| positions.len())
        {
            Some(positions) => Box::new(
                positions
                    .range(start..)
                    .map(move |position| &events[*position]),
            ),
            None => Box::new(events[start..].iter()),
        }
    }

    fn matches<E: LedgerEventRead>(&self, query: &EventQuery, event: &E, snapshot: u64) -> bool {
        query_matches_fields(query, event)
            && query.view.is_none_or(|view| {
                view.contains(
                    event.event_type(),
                    event.severity(),
                    event.origin().source(),
                    self.lab_related(event, snapshot),
                )
            })
    }

    pub(super) fn lab_related<E: LedgerEventRead>(&self, event: &E, snapshot: u64) -> bool {
        let links = event.links();
        let request_matches = |request: &RequestId| {
            self.lab_requests
                .get(request)
                .is_some_and(|sequence| *sequence <= snapshot)
        };
        let correlation_matches = |correlation: &CorrelationId| {
            self.lab_correlations
                .get(correlation)
                .is_some_and(|sequence| *sequence <= snapshot)
        };
        LAB_RELATIONS
            .into_iter()
            .any(|(axis, via_run)| match (axis, via_run) {
                (LabAxis::Request, false) => links.request_id().is_some_and(request_matches),
                (LabAxis::Correlation, false) => {
                    links.correlation_id().is_some_and(correlation_matches)
                }
                (LabAxis::Request, true) => links.run_id().is_some_and(|run| {
                    self.run_requests.get(run).is_some_and(|requests| {
                        requests.iter().any(|(request, sequence)| {
                            *sequence <= snapshot && request_matches(request)
                        })
                    })
                }),
                (LabAxis::Correlation, true) => links.run_id().is_some_and(|run| {
                    self.run_correlations.get(run).is_some_and(|correlations| {
                        correlations.iter().any(|(correlation, sequence)| {
                            *sequence <= snapshot && correlation_matches(correlation)
                        })
                    })
                }),
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
            "outcome_projection_not_ready",
            "project_scheduling_outcomes",
        ));
    }
    let [terminal] = terminals else {
        return Err(GlobalLedgerError::fatal(
            "outcome_projection_terminal_not_unique",
            "project_scheduling_outcomes",
        ));
    };
    let [admitted] = admissions else {
        return Err(GlobalLedgerError::fatal(
            "outcome_projection_admission_not_unique",
            "project_scheduling_outcomes",
        ));
    };
    let [lease_granted] = leases else {
        return Err(GlobalLedgerError::fatal(
            "outcome_projection_lease_not_unique",
            "project_scheduling_outcomes",
        ));
    };
    let EventPayload::Task(TaskPayload::Semantic(terminal_payload)) = terminal.payload() else {
        return Err(GlobalLedgerError::fatal(
            "outcome_projection_terminal_invalid",
            "project_scheduling_outcomes",
        ));
    };
    let TaskSemanticFact::TerminalCommitted {
        outcome: TaskOutcome::Success,
        failure_code: None,
        scheduling_disposition: Some(disposition),
        ..
    } = terminal_payload.fact()
    else {
        return Err(GlobalLedgerError::fatal(
            "outcome_projection_terminal_invalid",
            "project_scheduling_outcomes",
        ));
    };
    let EventPayload::Policy(PolicyPayload::DispatchAdmitted(admission)) = admitted.payload()
    else {
        return Err(GlobalLedgerError::fatal(
            "outcome_projection_admission_invalid",
            "project_scheduling_outcomes",
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
            "outcome_projection_identity_mismatch",
            "project_scheduling_outcomes",
        ));
    }
    let outcome = AuthoritativeSchedulingOutcome::new(
        expected.clone(),
        disposition.clone(),
        terminal.timestamp_unix_ms(),
    )
    .map_err(|_| {
        GlobalLedgerError::fatal(
            "outcome_projection_identity_invalid",
            "project_scheduling_outcomes",
        )
    })?;
    SchedulingOutcomeProjection::new(ledger_position, outcome).map_err(|_| {
        GlobalLedgerError::fatal("outcome_projection_invalid", "project_scheduling_outcomes")
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

fn query_matches_fields<E: LedgerEventRead>(query: &EventQuery, event: &E) -> bool {
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
