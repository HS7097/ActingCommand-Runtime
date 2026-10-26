// SPDX-License-Identifier: AGPL-3.0-only

//! LN-7 (#422, LEAN-L §3.5): what the writer keeps resident of the retention state. #422 LN-7c Q
//! (final-query §2): the frame cleaner decides by ledger query, so the writer keeps no frame
//! facts and reads no frame at its open. Besides the eviction proof set and the owner changes,
//! ends, imports and closes, it keeps the objects of the capture frames its records named since
//! the open, which `retain_live_objects` drops at each cleaner pass start unless the frame is
//! still in the live frame store. A record that names a run or a capture frame the writer does not
//! hold loads it again from the store, by its link or its `artifact_id` index, before the record
//! is checked; `apply` is fed those records unchanged, in ledger order.

use super::*;
use crate::global::history::{HistorySelection, LinkSet};
use crate::global::storage::{DurableStorage, EventStore};
use actingcommand_contract::EventQuery;
use std::path::Path;

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
            // #422 LN-7c Q: the open reads no frame record, so a sealed intent brings its own
            // object (the eviction proof set stays resident).
            if let ArtifactRetentionFact::EvictionIntent(intent) = fact
                && !self
                    .objects
                    .contains_key(&intent.identity.artifact.artifact_id)
            {
                self.proof_object(intent, event.sequence());
            }
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

    /// #422 LN-7c Q: the object of a sealed eviction intent whose records were not selected, as
    /// the intent records it (its identity and its verified source), so that the intent seals it.
    fn proof_object(
        &mut self,
        intent: &actingcommand_contract::ArtifactEvictionIntentRecord,
        sequence: u64,
    ) {
        let reference = intent.identity.artifact.clone();
        if let Some(frame) = reference.frame_id {
            self.frames
                .entry(frame)
                .or_default()
                .insert(reference.artifact_id);
            self.seen_frames.insert(frame);
        }
        self.objects.insert(
            reference.artifact_id,
            RetainedObject {
                reference,
                last_reference_sequence: sequence,
                verified: Some(intent.verified),
                verified_lab: LabKeys::default(),
                summary: intent.capture_summary,
                identity: Some(intent.identity.clone()),
                pins: BTreeMap::new(),
                released_lab_pins: BTreeMap::new(),
                reference_scopes: BTreeSet::new(),
                permanently_protected: false,
                proof: None,
            },
        );
    }

    /// #422 LN-7c Q (final-query §2): at each frame cleaner pass start, keeps of the capture frame
    /// objects only those of the frames in `live` (the live frame store's listing) and the
    /// eviction proof set. A dropped object is loaded again from the store when a record names it
    /// (`load_retention_sources`); nothing else of the index is dropped.
    pub(in crate::global) fn retain_live_objects(&mut self, live: &BTreeSet<ArtifactId>) {
        self.objects.retain(|id, object| {
            object.reference.kind != ArtifactKind::CaptureFrame
                || live.contains(id)
                || object.proof.is_some()
        });
        let objects = &self.objects;
        self.frames.retain(|_, ids| {
            ids.retain(|id| objects.contains_key(id));
            !ids.is_empty()
        });
        let frames = &self.frames;
        self.seen_frames.retain(|frame| frames.contains_key(frame));
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
            && !self.retention.known_runs.contains(&run)
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
    /// run without an earlier record. #422 LN-7c Q: either way the run is known from now on.
    fn load_run(&mut self, run: RunId, before: u64) -> GlobalLedgerResult<()> {
        let query = EventQuery {
            run_id: Some(run),
            to_sequence: Some(before),
            ..EventQuery::default()
        };
        let records = self.history().select(&HistorySelection::of(&query))?;
        self.retention.known_runs.insert(run);
        if records.is_empty() {
            return Ok(());
        }
        let mut loaded = RetentionIndex::scratch(&self.retention, records);
        let retention = &mut self.retention;
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
    /// hold.
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
            self.objects.insert(id, object);
        }
    }
}
