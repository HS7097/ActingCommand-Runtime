// SPDX-License-Identifier: AGPL-3.0-only

//! LN-7 (#422, LEAN-L §3.5): the retention state of a store opened anew, read by type and by
//! index, never from the whole history. LN-8 drops the open's full read on top of it.

use super::view::ERROR_WINDOW_MS;
use super::*;
use crate::global::history::{HistorySelection, LinkSet, check_deadline};
use crate::global::storage::{DurableStorage, EventStore};
use actingcommand_contract::{EventQuery, parse_resource_reading_snapshot_id};
use std::time::Instant;

/// The record types whose payload can name an owner epoch (`recorded_owner`): lifecycle
/// observations (with the prior-epoch imports and closes and the quiescences), capacity samples
/// and runtime-state facts.
pub(super) const OWNER_TYPES: [EventType; 5] = [
    EventType::RuntimeLifecycleObserved,
    EventType::PerformanceSummary,
    EventType::CommandValidated,
    EventType::MonitorProbeCompleted,
    EventType::MonitorProbeFailed,
];

/// The starts, the instance bindings and the eviction proof set, each read whole.
const RARE_TYPES: [EventType; 5] = [
    EventType::RuntimeStarted,
    EventType::RuntimeTakeover,
    EventType::RuntimeInstanceBound,
    EventType::ArtifactEvictionIntent,
    EventType::ArtifactEvictionOutcome,
];

/// The records an open selects, by sequence.
type Selected = BTreeMap<u64, PersistedEvent>;

fn keep(selected: &mut Selected, events: Vec<PersistedEvent>) {
    for event in events {
        selected.entry(event.sequence()).or_insert(event);
    }
}

fn typed(types: &[EventType]) -> EventQuery {
    EventQuery {
        event_types: types.to_vec(),
        ..EventQuery::default()
    }
}

/// Whether `event` is a `fact.published` naming one of `frames` through a record's snapshot id
/// (a resource reading's frame, an adjustment's confirmation frame) or one of `artifacts` as a
/// record's content: what `FrameFacts::apply` takes from it into `published_frames`,
/// `readings` and `published_artifacts`.
fn publishes_on(
    event: &PersistedEvent,
    frames: &BTreeSet<FrameId>,
    artifacts: &BTreeSet<ArtifactId>,
) -> bool {
    let EventPayload::Fact(FactPayload::Published(payload)) = event.payload() else {
        return false;
    };
    payload.records().any(|record| {
        parse_resource_reading_snapshot_id(&record.source_snapshot_id)
            .is_some_and(|(_, frame)| frames.contains(&frame))
            || matches!(&record.content, FactContent::Artifact { artifact }
                if artifacts.contains(&artifact.artifact_id))
    })
}

impl RetentionIndex {
    /// The sequence ranges in which `owner` is the recorded owner, the last one through `head`.
    pub(super) fn owner_ranges(&self, owner: OwnerEpoch, head: u64) -> Vec<(u64, u64)> {
        let mut ranges = Vec::new();
        let mut changes = self.owner_epochs.iter().peekable();
        while let Some((start, epoch)) = changes.next() {
            if *epoch == owner {
                let end = changes
                    .peek()
                    .map_or(head, |(next, _)| next.saturating_sub(1));
                ranges.push((*start, end));
            }
        }
        ranges
    }
}

impl<B: DurableStorage> EventStore<B> {
    /// The resident retention state of a store opened anew: the owner records (owner changes,
    /// ends, imports, closes and quiescences); the starts, bindings and eviction proof set; the
    /// scopes (grouped through the link index) and the `Performed` releases of the epochs whose
    /// closes a start may still write; and what the classes of the live and the proven frames
    /// read (`frame_records`). The records are applied in ledger order without `validate` (each
    /// was checked when it was written and is authenticated when it is read); then only the facts
    /// of `live` and what their classes read stay. LN-8d (#422): every read stops at `deadline`
    /// (the open's, `ledger_observer.open_timeout_ms`) with `ledger_read_budget_exceeded`.
    /// #422 LN-7c O: `dump_present`, a dump this open imports, makes it read every owner record.
    pub(in crate::global) fn open_retention(
        &mut self,
        live: &BTreeSet<ArtifactId>,
        dump_present: bool,
        deadline: Option<Instant>,
    ) -> GlobalLedgerResult<()> {
        let head = self.backend.head();
        // #422 LN-7c O: the starts are read first; the owner records are those of the two
        // latest owner epochs and of the older owners a start may still close (`owner_records`).
        let rare = self
            .history_until(deadline)
            .select(&HistorySelection::of(&typed(&RARE_TYPES)))?;
        let starts = rare
            .iter()
            .filter(|event| {
                matches!(
                    event.event_type(),
                    EventType::RuntimeStarted | EventType::RuntimeTakeover
                )
            })
            .map(PersistedEvent::sequence)
            .collect::<Vec<_>>();
        let super::owner_read::OwnerRead {
            records: owner_records,
            full,
        } = self.owner_records(&starts, dump_present, deadline)?;
        let owners = RetentionIndex::scratch(&RetentionIndex::default(), owner_records.clone());
        let mut selected = Selected::new();
        keep(&mut selected, owner_records);
        let mut frames = live.clone();
        frames.extend(rare.iter().filter_map(|event| {
            Some(
                event
                    .payload()
                    .artifact_retention()?
                    .identity()
                    .artifact
                    .artifact_id,
            )
        }));
        keep(&mut selected, rare);
        let mut extents = Vec::new();
        for owner in owners.retained_owners() {
            for range in owners.owner_ranges(owner, head) {
                check_deadline(deadline)?;
                extents.extend(
                    self.backend
                        .scope_extents(range)?
                        .into_iter()
                        .map(|extent| (owner, extent)),
                );
                let releases = EventQuery {
                    event_type: Some(EventType::LeaseReleased),
                    from_sequence: Some(range.0),
                    to_sequence: Some(range.1),
                    ..EventQuery::default()
                };
                keep(
                    &mut selected,
                    self.history_until(deadline)
                        .select(&HistorySelection::of(&releases))?,
                );
            }
        }
        let framed = self.frame_records(&frames, live, deadline)?;
        selected.extend(framed);
        let mut retention =
            RetentionIndex::scratch(&RetentionIndex::default(), selected.into_values().collect());
        for (owner, extent) in extents {
            retention.scope_sources.insert(
                ClosureScope {
                    owner,
                    instance: extent.instance,
                    run: extent.run,
                    lease: extent.lease,
                    request: extent.request,
                    correlation: extent.correlation,
                },
                (extent.first, extent.last),
            );
        }
        retention.through_sequence = head;
        retention.retain_frames(live);
        retention.owner_read_full = full;
        #[cfg(test)]
        retention.keep_owner_state_for_test(head);
        self.recovering_retention = retention.has_pending();
        self.retention = retention;
        Ok(())
    }

    /// The records the classes of `frames` read: those that reference them (by the
    /// `artifact_id` index) or carry their frame link; the records of the runs, leases, requests
    /// and correlations these name; the `fact.published` records naming a frame of `live` from
    /// the first record of those frames on (a resource reading or a confirmation is published
    /// with system links: no run, instance, lease or frame link, and inline content);
    /// the markers of their instances from the first of those records on (a marker preserving a
    /// frame names it in its payload only); the error points whose window can hold one of them
    /// (Warning or higher, and failed terminals, from the oldest capture through 30 s after the
    /// newest); then the records of the runs these name, of the marker neighbours' frames and of
    /// the scopes linked to the runs read; and once more the runs the neighbours' records name.
    fn frame_records(
        &self,
        frames: &BTreeSet<ArtifactId>,
        live: &BTreeSet<ArtifactId>,
        deadline: Option<Instant>,
    ) -> GlobalLedgerResult<Selected> {
        let mut selected = Selected::new();
        if frames.is_empty() {
            return Ok(selected);
        }
        keep(
            &mut selected,
            self.read_linked(
                LinkSet {
                    artifacts: frames.clone(),
                    ..LinkSet::default()
                },
                deadline,
            )?,
        );
        let mut frame_ids = BTreeSet::new();
        let mut live_frames = BTreeSet::new();
        let mut instances = BTreeSet::new();
        let mut captured = Vec::new();
        for event in selected.values() {
            for reference in material_references(event) {
                if frames.contains(&reference.artifact_id) {
                    frame_ids.extend(reference.frame_id);
                    captured.push(reference.created_at_unix_ms);
                }
                if live.contains(&reference.artifact_id) {
                    live_frames.extend(reference.frame_id);
                }
            }
            instances.extend(event.links().instance_id().copied());
        }
        let first = selected.keys().next().copied().unwrap_or(0);
        keep(
            &mut selected,
            self.read_linked(
                LinkSet {
                    frames: frame_ids.clone(),
                    ..LinkSet::default()
                },
                deadline,
            )?,
        );
        // The first record of the oldest live frame: no record before it can name a live frame.
        let first_live = selected
            .values()
            .find(|event| {
                event
                    .links()
                    .frame_id()
                    .is_some_and(|frame| live_frames.contains(frame))
                    || material_references(*event)
                        .iter()
                        .any(|reference| live.contains(&reference.artifact_id))
            })
            .map(PersistedEvent::sequence);
        let mut scoped = LinkSet::default();
        for event in selected.values() {
            let links = event.links();
            scoped.runs.extend(links.run_id().copied());
            scoped.leases.extend(links.lease_id().copied());
            scoped.requests.extend(links.request_id().copied());
            scoped.correlations.extend(links.correlation_id().copied());
        }
        let read = scoped.clone();
        keep(&mut selected, self.read_linked(scoped, deadline)?);
        if let Some(from) = first_live {
            let published = EventQuery {
                event_type: Some(EventType::FactPublished),
                from_sequence: Some(from),
                ..EventQuery::default()
            };
            keep(
                &mut selected,
                self.history_until(deadline)
                    .select_where(&HistorySelection::of(&published), |event| {
                        publishes_on(event, &live_frames, live)
                    })?,
            );
        }
        if !instances.is_empty() {
            let markers = EventQuery {
                event_type: Some(EventType::CaptureDedupWindow),
                instance_ids: instances.into_iter().collect(),
                from_sequence: Some(first),
                ..EventQuery::default()
            };
            keep(
                &mut selected,
                self.history_until(deadline)
                    .select(&HistorySelection::of(&markers))?,
            );
        }
        if let (Some(oldest), Some(newest)) = (captured.iter().min(), captured.iter().max()) {
            let through = newest.saturating_add(ERROR_WINDOW_MS).saturating_add(1);
            for points in [
                EventQuery {
                    minimum_severity: Some(EventSeverity::Warning),
                    from_timestamp_unix_ms: Some(*oldest),
                    to_timestamp_unix_ms: Some(through),
                    ..EventQuery::default()
                },
                EventQuery {
                    event_type: Some(EventType::TaskFailed),
                    from_timestamp_unix_ms: Some(*oldest),
                    to_timestamp_unix_ms: Some(through),
                    ..EventQuery::default()
                },
            ] {
                keep(
                    &mut selected,
                    self.history_until(deadline)
                        .select(&HistorySelection::of(&points))?,
                );
            }
        }
        let mut more = LinkSet::default();
        for event in selected.values() {
            let links = event.links();
            if let Some(run) = links.run_id() {
                if read.runs.contains(run) {
                    more.requests.extend(
                        links
                            .request_id()
                            .filter(|request| !read.requests.contains(*request))
                            .copied(),
                    );
                    more.correlations.extend(
                        links
                            .correlation_id()
                            .filter(|correlation| !read.correlations.contains(*correlation))
                            .copied(),
                    );
                } else {
                    more.runs.insert(*run);
                }
            }
            if let EventPayload::Capture(CapturePayload::DedupWindow(window)) = event.payload()
                && let (Some(preserved), Some(representative)) =
                    (window.preserved_frame_id(), links.frame_id())
            {
                for (frame, other) in [(preserved, representative), (representative, preserved)] {
                    if frame_ids.contains(frame) && !frame_ids.contains(other) {
                        more.frames.insert(*other);
                    }
                }
            }
        }
        // A marker neighbour's records name its run, which nothing read before may have (a
        // chain across runs): that run's terminal and summary are read too, or its frames would
        // get an epoch-end point the full view does not have.
        let mut runs_read = read.runs;
        runs_read.extend(more.runs.iter().copied());
        let neighboured = self.read_linked(more, deadline)?;
        let further = LinkSet {
            runs: neighboured
                .iter()
                .filter_map(|event| event.links().run_id())
                .filter(|run| !runs_read.contains(*run))
                .copied()
                .collect(),
            ..LinkSet::default()
        };
        keep(&mut selected, neighboured);
        keep(&mut selected, self.read_linked(further, deadline)?);
        Ok(selected)
    }

    /// The committed records linked to one of `links`' identifiers, each dimension read through
    /// its own index. LN-8d (#422): one read of the whole set, each page the first rows of the
    /// union of its dimensions (`linked_page_sequences` in `global/sqlite.rs`), so a record linked
    /// in several dimensions (a run's records carry its run, lease, request and correlation) is
    /// read and authenticated once; before `deadline`.
    fn read_linked(
        &self,
        links: LinkSet,
        deadline: Option<Instant>,
    ) -> GlobalLedgerResult<Vec<PersistedEvent>> {
        if links.is_empty() {
            return Ok(Vec::new());
        }
        self.history_until(deadline)
            .select(&HistorySelection::linked(&EventQuery::default(), &links))
    }
}
