// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{EventQuery, PersistedEvent, ProjectedEvent, ProjectionProfile};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub(super) struct EventIndexes {
    event_ids: BTreeMap<String, usize>,
    instance_ids: BTreeMap<String, BTreeSet<usize>>,
    request_ids: BTreeMap<String, BTreeSet<usize>>,
    correlation_ids: BTreeMap<String, BTreeSet<usize>>,
    causation_ids: BTreeMap<String, BTreeSet<usize>>,
    task_ids: BTreeMap<String, BTreeSet<usize>>,
    run_ids: BTreeMap<String, BTreeSet<usize>>,
    lease_ids: BTreeMap<String, BTreeSet<usize>>,
    frame_ids: BTreeMap<String, BTreeSet<usize>>,
    action_ids: BTreeMap<String, BTreeSet<usize>>,
    reco_ids: BTreeMap<String, BTreeSet<usize>>,
}

impl EventIndexes {
    pub(super) fn from_events(events: &[PersistedEvent]) -> Self {
        let mut indexes = Self::default();
        for (position, event) in events.iter().enumerate() {
            indexes.insert(event, position);
        }
        indexes
    }

    pub(super) fn contains_event_id(&self, event_id: &str) -> bool {
        self.event_ids.contains_key(event_id)
    }

    pub(super) fn insert(&mut self, event: &PersistedEvent, position: usize) {
        self.event_ids.insert(event.event_id.clone(), position);
        insert_link(&mut self.instance_ids, &event.links.instance_id, position);
        insert_link(&mut self.request_ids, &event.links.request_id, position);
        insert_link(
            &mut self.correlation_ids,
            &event.links.correlation_id,
            position,
        );
        insert_link(&mut self.causation_ids, &event.links.causation_id, position);
        insert_link(&mut self.task_ids, &event.links.task_id, position);
        insert_link(&mut self.run_ids, &event.links.run_id, position);
        insert_link(&mut self.lease_ids, &event.links.lease_id, position);
        insert_link(&mut self.frame_ids, &event.links.frame_id, position);
        insert_link(&mut self.action_ids, &event.links.action_id, position);
        insert_link(&mut self.reco_ids, &event.links.reco_id, position);
    }

    pub(super) fn query(
        &self,
        events: &[PersistedEvent],
        query: &EventQuery,
    ) -> Vec<PersistedEvent> {
        let candidates = [
            index_for(&self.instance_ids, &query.instance_id),
            index_for(&self.request_ids, &query.request_id),
            index_for(&self.correlation_ids, &query.correlation_id),
            index_for(&self.causation_ids, &query.causation_id),
            index_for(&self.task_ids, &query.task_id),
            index_for(&self.run_ids, &query.run_id),
            index_for(&self.lease_ids, &query.lease_id),
            index_for(&self.frame_ids, &query.frame_id),
            index_for(&self.action_ids, &query.action_id),
            index_for(&self.reco_ids, &query.reco_id),
        ];
        let positions = candidates.into_iter().flatten().fold(
            None,
            |current: Option<BTreeSet<usize>>, matches| {
                Some(match current {
                    Some(current) => current.intersection(matches).copied().collect(),
                    None => matches.clone(),
                })
            },
        );
        let events: Box<dyn Iterator<Item = &PersistedEvent>> = match positions {
            Some(positions) => Box::new(positions.into_iter().map(|position| &events[position])),
            None => Box::new(events.iter()),
        };
        events
            .filter(|event| query.matches(event))
            .cloned()
            .collect()
    }
}

pub(super) fn project(event: &PersistedEvent, profile: ProjectionProfile) -> ProjectedEvent {
    let payload = match profile {
        ProjectionProfile::Cli | ProjectionProfile::Concise => None,
        ProjectionProfile::Ui
        | ProjectionProfile::Normal
        | ProjectionProfile::Lab
        | ProjectionProfile::Verbose
        | ProjectionProfile::Forensic => Some(event.payload.clone()),
    };
    ProjectedEvent {
        sequence: event.sequence,
        event_id: event.event_id.clone(),
        timestamp_unix_ms: event.timestamp_unix_ms,
        event_type: event.event_type,
        severity: event.severity,
        origin: event.origin.clone(),
        links: event.links.clone(),
        payload_schema: event.payload_schema.clone(),
        payload,
        artifacts: event.artifacts.clone(),
    }
}

fn insert_link(
    index: &mut BTreeMap<String, BTreeSet<usize>>,
    value: &Option<String>,
    position: usize,
) {
    if let Some(value) = value {
        index.entry(value.clone()).or_default().insert(position);
    }
}

fn index_for<'a>(
    index: &'a BTreeMap<String, BTreeSet<usize>>,
    value: &Option<String>,
) -> Option<&'a BTreeSet<usize>> {
    value.as_ref().and_then(|value| index.get(value))
}
