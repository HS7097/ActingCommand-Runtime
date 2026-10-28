// SPDX-License-Identifier: AGPL-3.0-only

//! LN-7 (#422, LEAN-L §3.5): what the writer keeps resident of the retention state. #422 LN-7c Q
//! (final-query §2): the frame cleaner decides by ledger query, so the writer keeps no frame
//! facts and reads no frame at its open. Besides the eviction proof set and the owner changes,
//! ends, imports and closes, it keeps the objects of the capture frames its records named since
//! the open, which `retain_live_objects` drops at each cleaner pass start unless the frame is
//! still in the live frame store, and with them the run- and scope-keyed facts of the runs and
//! scopes no kept object or open run needs (review F5). A record that names a run or a capture
//! frame the writer does not hold loads it again from the store, by its link or its
//! `artifact_id` index, before the record is checked; `apply` is fed those records unchanged, in
//! ledger order.

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
    /// (`load_retention_sources`).
    ///
    /// Review F5 (fix round 1): the run- and scope-keyed facts are trimmed as LN-7's
    /// `retain_frames` trimmed them, to the retained objects and owners. Kept are the runs of the
    /// retained objects and the runs a record was applied or loaded of that have no terminal
    /// yet (the open runs this writer knows); the scopes of the retained objects, those runs'
    /// scopes and the sealed scopes; and the closes, scope extents and lease releases of the
    /// owners a start may still close and of the retained objects' identities. A dropped run
    /// leaves `known_runs` with its facts, so its next record loads it again from the store
    /// (`load_run`), as LN-7's dropped runs were.
    ///
    /// #422 QR-1: the trim runs in bounded chunks, one writer command each, so event writes and
    /// reads go on between them: [`Self::retain_live_objects_chunk`] looks at no more than a
    /// chunk's objects, and the chunk that reaches the last object ends with
    /// [`Self::trim_to_retained_objects`].
    pub(in crate::global) fn retain_live_objects_chunk(
        &mut self,
        live: &BTreeSet<ArtifactId>,
        (after, as_of): (Option<ArtifactId>, u64),
        max_objects: usize,
    ) -> LiveFramesChunk {
        use std::ops::Bound::{Excluded, Unbounded};
        let bounds = (after.map_or(Unbounded, Excluded), Unbounded);
        let max_objects = max_objects.max(1);
        let mut looked = 0;
        let mut last = None;
        let mut dropped = Vec::new();
        for (id, object) in self.objects.range(bounds).take(max_objects) {
            looked += 1;
            last = Some(*id);
            // An object a record named after the pass start (`as_of`) is the newer state's: it
            // is not one the pass start's listing could hold, and it stays.
            if object.reference.kind == ArtifactKind::CaptureFrame
                && !live.contains(id)
                && object.proof.is_none()
                && object.last_reference_sequence <= as_of
            {
                dropped.push((*id, object.reference.frame_id));
            }
        }
        for (id, frame) in &dropped {
            self.objects.remove(id);
            if let Some(frame) = frame
                && let Some(ids) = self.frames.get_mut(frame)
            {
                ids.remove(id);
            }
        }
        let more = looked == max_objects
            && last.is_some_and(|last| {
                self.objects
                    .range((Excluded(last), Unbounded))
                    .next()
                    .is_some()
            });
        if !more {
            self.trim_to_retained_objects();
        }
        let next = if more {
            last.map(|after| LiveFramesCursor { after, as_of })
        } else {
            None
        };
        LiveFramesChunk {
            looked,
            dropped: dropped.len(),
            next,
        }
    }

    /// #422 QR-1, tests: `count` new capture frame objects, each of its own frame, as a record at
    /// `sequence` would name them (none proven); their ids, in this index's order.
    #[cfg(test)]
    pub(in crate::global) fn seed_frame_objects_for_test(
        &mut self,
        count: usize,
        sequence: u64,
    ) -> Vec<ArtifactId> {
        let ids = actingcommand_contract::IdentifierIssuer::new().expect("identifier issuer");
        let mut seeded = Vec::with_capacity(count);
        for _ in 0..count {
            let artifact = *ids.mint_artifact_id().expect("artifact id").transport();
            let frame = *ids.mint_frame_id().expect("frame id").transport();
            let reference = ProjectedArtifactReference {
                artifact_id: artifact,
                kind: ArtifactKind::CaptureFrame,
                run_id: None,
                frame_id: Some(frame),
                correlation_id: None,
                object_key: None,
                media_type: ArtifactKind::CaptureFrame.media_type(),
                byte_count: 1,
                sha256: String::new(),
                created_at_unix_ms: 0,
                producer: actingcommand_contract::ArtifactProducer::CaptureStore,
                retention_class: actingcommand_contract::RetentionClass::Adaptive,
                redaction_state: actingcommand_contract::ArtifactRedactionState::NotRequired,
            };
            self.frames.entry(frame).or_default().insert(artifact);
            self.seen_frames.insert(frame);
            self.objects.insert(
                artifact,
                RetainedObject {
                    reference,
                    last_reference_sequence: sequence,
                    verified: None,
                    verified_lab: LabKeys::default(),
                    summary: None,
                    identity: None,
                    pins: BTreeMap::new(),
                    released_lab_pins: BTreeMap::new(),
                    reference_scopes: BTreeSet::new(),
                    permanently_protected: false,
                    proof: None,
                },
            );
            seeded.push(artifact);
        }
        seeded.sort();
        seeded
    }

    /// #422 QR-1: the end of a pass start's trim, after its last chunk: the frames of the
    /// objects no longer held, and the run- and scope-keyed facts no retained object or open run
    /// needs (review F5).
    fn trim_to_retained_objects(&mut self) {
        let objects = &self.objects;
        self.frames.retain(|_, ids| {
            ids.retain(|id| objects.contains_key(id));
            !ids.is_empty()
        });
        let frames = &self.frames;
        self.seen_frames.retain(|frame| frames.contains_key(frame));
        let mut runs = self
            .known_runs
            .iter()
            .filter(|run| !self.run_terminals.contains_key(*run))
            .copied()
            .collect::<BTreeSet<_>>();
        let mut scopes = self.sealed_scopes.clone();
        let mut identities = BTreeSet::new();
        let mut lease_keys = BTreeSet::new();
        for object in self.objects.values() {
            scopes.extend(object.reference_scopes.iter().copied());
            if let Some(identity) = object.identity.as_ref() {
                runs.extend(identity.run_id);
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
        self.known_runs.retain(|run| runs.contains(run));
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
            // #422 QR-1: a proof taken in is a change of the proofs the reader pool reads; #422
            // W1-8 (P27): it joins the persistent proof set too.
            if let Some(proof) = &object.proof {
                self.proof_changed(id, proof.clone());
            }
            self.objects.insert(id, object);
        }
    }
}
