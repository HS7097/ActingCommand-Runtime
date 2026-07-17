// SPDX-License-Identifier: AGPL-3.0-only

use crate::PersistedEvent;
use actingcommand_contract::{
    ActionId, CausationId, CorrelationId, EventId, EventQuery, EventType, FrameId, InstanceId,
    LeaseId, ProjectedEvent, ProjectionPayload, ProjectionProfile, RecognitionId, RequestId, RunId,
    TaskId,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub(super) struct EventIndexes {
    event_ids: BTreeMap<EventId, usize>,
    event_types: BTreeMap<usize, BTreeSet<usize>>,
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
}

impl EventIndexes {
    pub(super) fn from_events(events: &[PersistedEvent]) -> Self {
        let mut indexes = Self::default();
        for (position, event) in events.iter().enumerate() {
            indexes.insert(event, position);
        }
        indexes
    }

    pub(super) fn contains_event_id(&self, event_id: &EventId) -> bool {
        self.event_ids.contains_key(event_id)
    }

    pub(super) fn insert(&mut self, event: &PersistedEvent, position: usize) {
        self.event_ids.insert(*event.event_id(), position);
        self.event_types
            .entry(event_type_index(event.event_type()))
            .or_default()
            .insert(position);
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
    }

    pub(super) fn query(
        &self,
        events: &[PersistedEvent],
        query: &EventQuery,
    ) -> Vec<PersistedEvent> {
        let minimum_sequence = query.from_sequence.unwrap_or(0);
        let start = events.partition_point(|event| event.sequence() < minimum_sequence);
        self.candidates_from(events, query, start)
            .filter(|event| query_matches(query, event))
            .cloned()
            .collect()
    }

    pub(super) fn query_page(
        &self,
        events: &[PersistedEvent],
        query: &EventQuery,
        after_sequence: u64,
        through_sequence: u64,
        page_events: usize,
    ) -> Vec<PersistedEvent> {
        self.query_page_with_observer(
            events,
            query,
            after_sequence,
            through_sequence,
            page_events,
            &mut || {},
        )
    }

    fn query_page_with_observer(
        &self,
        events: &[PersistedEvent],
        query: &EventQuery,
        after_sequence: u64,
        through_sequence: u64,
        page_events: usize,
        observe_candidate: &mut impl FnMut(),
    ) -> Vec<PersistedEvent> {
        let minimum_sequence = query
            .from_sequence
            .unwrap_or(0)
            .max(after_sequence.saturating_add(1));
        let start = events.partition_point(|event| event.sequence() < minimum_sequence);
        self.candidates_from(events, query, start)
            .take_while(|event| event.sequence() <= through_sequence)
            .filter(|event| {
                observe_candidate();
                event.sequence() > after_sequence && query_matches(query, event)
            })
            .take(page_events)
            .cloned()
            .collect()
    }

    #[cfg(test)]
    pub(super) fn query_page_with_visit_count(
        &self,
        events: &[PersistedEvent],
        query: &EventQuery,
        after_sequence: u64,
        through_sequence: u64,
        page_events: usize,
    ) -> (Vec<PersistedEvent>, usize) {
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

    fn candidates_from<'a>(
        &'a self,
        events: &'a [PersistedEvent],
        query: &EventQuery,
        start: usize,
    ) -> Box<dyn Iterator<Item = &'a PersistedEvent> + 'a> {
        let event_type = query.event_type.map(event_type_index);
        let candidates = [
            indexed_filter(&self.event_types, event_type.as_ref()),
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
}

pub(super) fn project(event: &PersistedEvent, profile: ProjectionProfile) -> ProjectedEvent {
    let (payload, include_object_key) = match profile {
        ProjectionProfile::Cli | ProjectionProfile::Concise => (ProjectionPayload::Omitted, false),
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
        artifacts: event
            .artifacts()
            .iter()
            .map(|artifact| artifact.project(include_object_key))
            .collect(),
    }
}

pub(super) fn project_if_matches(
    event: &PersistedEvent,
    query: &EventQuery,
    profile: ProjectionProfile,
) -> Option<ProjectedEvent> {
    query_matches(query, event).then(|| project(event, profile))
}

fn query_matches(query: &EventQuery, event: &PersistedEvent) -> bool {
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
            .source
            .is_none_or(|value| event.origin().source() == value)
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
