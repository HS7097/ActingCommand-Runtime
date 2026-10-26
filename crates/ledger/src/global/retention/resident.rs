// SPDX-License-Identifier: AGPL-3.0-only

//! LN-7 (#422, LEAN-L §3.5): what the writer keeps resident of the retention state. Besides the
//! eviction proof set and the owner changes, ends, imports and closes, it keeps the facts of the
//! frames present in the live frame store, and what their classes read: their runs (with every
//! frame of each, whose span dates waits, late and epoch-end points), the runs still open, the
//! marker neighbours, the error points whose window can hold a live frame and the scopes of their
//! Lab protection. `apply` keeps them current; `retain_frames` drops the rest when the frame
//! cleaner lists the live frames. A record that names a run or a capture frame the writer no
//! longer holds loads it again from the store, by its link or its `artifact_id` index, before the
//! record is checked; `classify` and `apply` are fed those records unchanged, in ledger order.

use super::view::ERROR_WINDOW_MS;
use super::*;
use crate::global::history::{HistorySelection, LinkSet};
use crate::global::storage::{DurableStorage, EventStore};
use actingcommand_contract::EventQuery;
use std::path::Path;
use std::sync::PoisonError;

/// LN-7: the capture frames present in the live frame store under `root`
/// (`artifacts\<shard>\artifact_<hex>.png`), by artifact id. A file whose name is no artifact id
/// is not an object of the store and is skipped; a listing failure is returned.
pub fn live_frame_ids(root: &Path) -> std::io::Result<BTreeSet<ArtifactId>> {
    Ok(actingcommand_artifact_store::list_live_frames(root)?
        .into_iter()
        .filter_map(|id| serde_json::from_value(serde_json::Value::String(id)).ok())
        .collect())
}

impl RetentionIndex {
    /// LN-7: an index of `records` alone, deduplicated and in ledger order, over the owner
    /// changes of `seed`. `validate` is not run: each record was checked when it was written and
    /// is authenticated when it is read.
    pub(super) fn scratch(seed: &Self, mut records: Vec<PersistedEvent>) -> Self {
        records.sort_by_key(PersistedEvent::sequence);
        records.dedup_by_key(|event| event.sequence());
        let mut index = Self {
            owner_epochs: seed.owner_epochs.clone(),
            owner_ends: seed.owner_ends.clone(),
            reopened_owners: seed.reopened_owners.clone(),
            ..Self::default()
        };
        for event in &records {
            index.apply_selected(event);
        }
        index
    }

    /// LN-7: applies one selected record as `apply` applies it after the record before it. The
    /// owner changes are already known, so an owner record changes nothing it would not; a
    /// retention fact whose object (or, for an eviction outcome, whose intent) was not selected
    /// has nothing to apply to.
    pub(super) fn apply_selected<E: LedgerEventRead>(&mut self, event: &E) {
        if let Some(fact) = event.payload().artifact_retention() {
            let Some(object) = self.objects.get(&fact.identity().artifact.artifact_id) else {
                return;
            };
            if matches!(fact, ArtifactRetentionFact::EvictionOutcome(_)) && object.proof.is_none() {
                return;
            }
        }
        self.through_sequence = event.sequence().saturating_sub(1);
        self.apply(event);
    }

    /// LN-7: keeps what the frame view needs for the frames in `live` (the capture frames whose
    /// object is in the live frame store) and drops the rest. Kept besides the live frames: every
    /// frame of a run that has a live frame, is still open, has a frame captured at most 30 s
    /// after a live frame of its instance (its epoch-end or late point can be dated there) or has
    /// a kept error point; the marker neighbours of the live frames; the error points whose
    /// window can hold a live frame; the Lab scopes of the kept frames and runs; the scopes a
    /// start may still close; and the eviction proof set. The kept frames that are not live are
    /// not listed by the view.
    pub(in crate::global) fn retain_frames(&mut self, live: &BTreeSet<ArtifactId>) {
        let listed = |object: &RetainedObject| {
            object.reference.kind == ArtifactKind::CaptureFrame && object.proof.is_none()
        };
        let mut live_objects = BTreeSet::new();
        let mut live_frames = BTreeSet::new();
        let mut live_at = BTreeMap::<InstanceId, Vec<u64>>::new();
        let mut runs = BTreeSet::new();
        for (id, object) in &self.objects {
            let Some(identity) = object
                .identity
                .as_ref()
                .filter(|_| listed(object) && live.contains(id))
            else {
                continue;
            };
            live_objects.insert(*id);
            live_frames.extend(object.reference.frame_id);
            live_at
                .entry(identity.instance_id)
                .or_default()
                .push(identity.artifact.created_at_unix_ms);
            runs.extend(identity.run_id);
        }
        for times in live_at.values_mut() {
            times.sort_unstable();
        }
        let near_live = |instance: &InstanceId, at: u64| {
            live_at.get(instance).is_some_and(|times| {
                let start =
                    times.partition_point(|time| *time < at.saturating_sub(ERROR_WINDOW_MS));
                times.get(start).is_some_and(|time| *time <= at)
            })
        };
        for object in self.objects.values() {
            if listed(object)
                && let Some(identity) = object.identity.as_ref()
                && let Some(run) = identity.run_id
                && near_live(&identity.instance_id, identity.artifact.created_at_unix_ms)
            {
                runs.insert(run);
            }
        }
        runs.extend(self.frame_facts.open_runs());
        let point_runs =
            self.frame_facts
                .retain_points(&live_at, &live_objects, &live_frames, &runs);
        runs.extend(point_runs);
        let neighbours = self.frame_facts.neighbours(&live_frames);
        let kept = self
            .objects
            .iter()
            .filter(|(id, object)| {
                live.contains(*id)
                    || object.proof.is_some()
                    || object
                        .identity
                        .as_ref()
                        .and_then(|identity| identity.run_id)
                        .is_some_and(|run| runs.contains(&run))
                    || object
                        .reference
                        .frame_id
                        .is_some_and(|frame| neighbours.contains(&frame))
            })
            .map(|(id, _)| *id)
            .collect::<BTreeSet<_>>();
        let mut frames = BTreeSet::new();
        let mut leases = BTreeSet::new();
        let mut scopes = self.sealed_scopes.clone();
        let mut identities = BTreeSet::new();
        let mut lease_keys = BTreeSet::new();
        for object in kept.iter().filter_map(|id| self.objects.get(id)) {
            frames.extend(object.reference.frame_id);
            scopes.extend(object.reference_scopes.iter().copied());
            if let Some(identity) = object.identity.as_ref() {
                leases.extend(identity.lease_id);
                identities.insert(ClosureScope::from_identity(identity));
                if let Some(lease) = identity.lease_id {
                    lease_keys.insert((identity.owner_epoch, identity.instance_id, lease));
                }
            }
        }
        for run in &runs {
            scopes.insert(RetentionScope::Run(*run));
            if let Some(linked) = self.run_scopes.get(run) {
                scopes.extend(linked.keys().copied());
            }
        }
        let sealed_runs = self
            .sealed_scopes
            .iter()
            .filter_map(|scope| match scope {
                RetentionScope::Run(run) => Some(*run),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        let owners = self.retained_owners();
        let closing =
            |scope: &ClosureScope| owners.contains(&scope.owner) || identities.contains(scope);
        self.objects.retain(|id, _| kept.contains(id));
        self.frames.retain(|frame, ids| {
            ids.retain(|id| kept.contains(id));
            frames.contains(frame) && !ids.is_empty()
        });
        self.seen_frames.retain(|frame| frames.contains(frame));
        self.recent.retain(|scope, _| scopes.contains(scope));
        self.linked_runs.retain(|scope, linked| {
            linked.retain(|run| runs.contains(run) || sealed_runs.contains(run));
            !linked.is_empty()
                && (scopes.contains(scope) || linked.iter().any(|run| sealed_runs.contains(run)))
        });
        self.run_scopes.retain(|run, _| runs.contains(run));
        self.lab_anchors.retain(|scope, _| scopes.contains(scope));
        self.material_scope_activity
            .retain(|scope, _| scopes.contains(scope));
        self.scheduled_runs.retain(|run| runs.contains(run));
        self.run_terminals.retain(|run, _| runs.contains(run));
        self.scope_sources.retain(|scope, _| closing(scope));
        self.closures.retain(|scope, _| closing(scope));
        self.prior_epoch_closes.retain(|scope, _| closing(scope));
        self.released_leases
            .retain(|key, _| owners.contains(&key.0) || lease_keys.contains(key));
        self.frame_facts.retain(&frames, &kept, &runs, &leases);
        self.ghosts = kept.into_iter().filter(|id| !live.contains(id)).collect();
        *self
            .frame_cache
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner) = None;
    }

    /// LN-7: the owners whose scopes a start may still close (`prior_epoch_owners`): the latest
    /// owner and the one before it, and each owner whose import or a scope close of it one of
    /// those two wrote.
    pub(super) fn retained_owners(&self) -> BTreeSet<OwnerEpoch> {
        let mut changes = self.owner_epochs.values().rev().copied();
        let mut owners = BTreeSet::new();
        if let Some(latest) = changes.next() {
            owners.insert(latest);
            if let Some(previous) = changes.find(|epoch| *epoch != latest) {
                owners.insert(previous);
            }
        }
        let writers = owners.clone();
        owners.extend(
            self.prior_epoch_writers
                .iter()
                .filter(|(_, writer)| writers.contains(*writer))
                .map(|(subject, _)| *subject),
        );
        owners
    }
}

impl<B: DurableStorage> EventStore<B> {
    /// LN-7: before `event` is checked, loads again what it names that the writer does not hold:
    /// each capture frame it references or records the retention of (its object, read by the
    /// `artifact_id` index and its frame link), and its run (read by the run link). A frame or a
    /// run the store has no earlier record of is new and loads nothing.
    pub(in crate::global) fn load_retention_sources(
        &mut self,
        event: &PersistedEvent,
    ) -> GlobalLedgerResult<()> {
        let before = event.sequence().saturating_sub(1);
        let mut wanted = material_references(event)
            .into_iter()
            .filter(|reference| reference.kind == ArtifactKind::CaptureFrame)
            .map(|reference| (reference.artifact_id, reference.frame_id))
            .collect::<BTreeMap<_, _>>();
        if let Some(fact) = event.payload().artifact_retention() {
            let artifact = &fact.identity().artifact;
            wanted
                .entry(artifact.artifact_id)
                .or_insert(artifact.frame_id);
        }
        wanted.retain(|id, _| !self.retention.objects.contains_key(id));
        if !wanted.is_empty() {
            self.load_objects(&wanted, before)?;
        }
        if let Some(run) = event.links().run_id().copied()
            && !self.retention.frame_facts.knows_run(&run)
        {
            self.load_run(run, before)?;
        }
        Ok(())
    }

    /// LN-7: the objects of `wanted` as the records through `before` that reference them or
    /// carry their frame link build them, as frames that left the live store.
    pub(in crate::global) fn load_objects(
        &mut self,
        wanted: &BTreeMap<ArtifactId, Option<FrameId>>,
        before: u64,
    ) -> GlobalLedgerResult<()> {
        let query = EventQuery {
            to_sequence: Some(before),
            ..EventQuery::default()
        };
        let referencing = LinkSet {
            artifacts: wanted.keys().copied().collect(),
            ..LinkSet::default()
        };
        let mut records = self
            .history()
            .select(&HistorySelection::linked(&query, &referencing))?;
        if records.is_empty() {
            return Ok(());
        }
        let mut framed = LinkSet {
            frames: wanted.values().flatten().copied().collect(),
            ..LinkSet::default()
        };
        for record in &records {
            framed.frames.extend(
                material_references(record)
                    .into_iter()
                    .filter(|reference| wanted.contains_key(&reference.artifact_id))
                    .filter_map(|reference| reference.frame_id),
            );
        }
        if !framed.frames.is_empty() {
            records.extend(
                self.history()
                    .select(&HistorySelection::linked(&query, &framed))?,
            );
        }
        let loaded = RetentionIndex::scratch(&self.retention, records);
        self.retention
            .adopt_objects(loaded, |id, _| wanted.contains_key(id));
        Ok(())
    }

    /// LN-7: `run`'s facts and frames as its records through `before` build them; nothing for a
    /// run without an earlier record.
    fn load_run(&mut self, run: RunId, before: u64) -> GlobalLedgerResult<()> {
        let query = EventQuery {
            run_id: Some(run),
            to_sequence: Some(before),
            ..EventQuery::default()
        };
        let records = self.history().select(&HistorySelection::of(&query))?;
        if records.is_empty() {
            return Ok(());
        }
        let mut loaded = RetentionIndex::scratch(&self.retention, records);
        let leases = loaded
            .objects
            .values()
            .filter_map(|object| object.identity.as_ref())
            .filter(|identity| identity.run_id == Some(run))
            .filter_map(|identity| identity.lease_id)
            .collect::<BTreeSet<_>>();
        let retention = &mut self.retention;
        retention
            .frame_facts
            .adopt_run(&mut loaded.frame_facts, &run, &leases);
        if let Some(terminal) = loaded.run_terminals.remove(&run) {
            retention.run_terminals.entry(run).or_insert(terminal);
        }
        if let Some(scopes) = loaded.run_scopes.remove(&run) {
            retention.run_scopes.entry(run).or_insert(scopes);
        }
        if loaded.scheduled_runs.contains(&run) {
            retention.scheduled_runs.insert(run);
        }
        for (scope, runs) in std::mem::take(&mut loaded.linked_runs) {
            if runs.contains(&run) {
                retention.linked_runs.entry(scope).or_default().insert(run);
            }
        }
        retention.adopt_objects(loaded, |_, object| {
            object
                .identity
                .as_ref()
                .is_some_and(|identity| identity.run_id == Some(run))
        });
        Ok(())
    }
}

impl RetentionIndex {
    /// LN-7: takes in the objects of `loaded` that `take` names and that this index does not
    /// hold, as frames that left the live store.
    fn adopt_objects(
        &mut self,
        loaded: RetentionIndex,
        take: impl Fn(&ArtifactId, &RetainedObject) -> bool,
    ) {
        for (id, object) in loaded.objects {
            if self.objects.contains_key(&id) || !take(&id, &object) {
                continue;
            }
            if let Some(frame) = object.reference.frame_id {
                self.frames.entry(frame).or_default().insert(id);
                self.seen_frames.insert(frame);
            }
            self.ghosts.insert(id);
            self.objects.insert(id, object);
            // A frame taken in can stand beside a live one: the classes are made again.
            *self
                .frame_cache
                .get_mut()
                .unwrap_or_else(PoisonError::into_inner) = None;
        }
    }
}
