// SPDX-License-Identifier: AGPL-3.0-only

//! LN-7 (#422, LEAN-L §3.5): the retention state of a store opened anew, read by type and by
//! index, never from the whole history. LN-8 drops the open's full read on top of it. #422 LN-7c Q
//! (final-query §2): the open reads no frame and does not list the live frame store; the frame
//! cleaner decides by ledger query.

use super::*;
use crate::global::history::{HistorySelection, check_deadline};
use crate::global::storage::{DurableStorage, EventStore};
use actingcommand_contract::EventQuery;
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
    /// ends, imports, closes and quiescences); the starts, bindings and eviction proof set; and the
    /// scopes (grouped through the link index) and the `Performed` releases of the epochs whose
    /// closes a start may still write. The records are applied in ledger order without `validate`
    /// (each was checked when it was written and is authenticated when it is read). LN-8d (#422):
    /// every read stops at `deadline` (the open's, `ledger_observer.open_timeout_ms`) with
    /// `ledger_read_budget_exceeded`. #422 LN-7c O: `dump_present`, a dump this open imports,
    /// makes it read every owner record. #422 LN-7c Q: no frame is read (the frame cleaner
    /// decides by ledger query); a sealed eviction intent brings its own object, so the proof set
    /// stays resident, and a record of a run or a frame loads them when it is appended.
    pub(in crate::global) fn open_retention(
        &mut self,
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
        // #422 LN-7c Q: the runs of the selected records were not read whole; a record of one of
        // them loads its run's earlier records when it is appended.
        retention.known_runs.clear();
        retention.owner_read_full = full;
        #[cfg(test)]
        retention.keep_owner_state_for_test(head);
        self.recovering_retention = retention.has_pending();
        self.retention = retention;
        Ok(())
    }
}
