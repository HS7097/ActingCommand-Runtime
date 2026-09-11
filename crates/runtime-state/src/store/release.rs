// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::{
    EventActor, EventPayload, EventSource, OriginModule, ReleasePayload, StatePayload,
};
use actingcommand_ledger::{PersistedEvent, ReleaseLedgerSourceReference};
use actingcommand_runtime_database::RuntimeTransaction;
use serde::{Deserialize, Serialize};

pub use actingcommand_ledger::RELEASE_BASELINE_STATE_KEY;
const MEMBERS: &str = "release.legacy.members.v1";
const SOURCES: &str = "release.sources.v1";
const FROM_SCHEMA: &str = "release.state.v1";
const TO_SCHEMA: &str = "release.atomic.v1";
// Bound both enumeration and retained metadata; individual entries retain the State limit.
const MAX_MEMBERS: usize = MAX_STATE_DOCUMENT_BYTES / 64;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum ReleaseBaselineMember {
    Generation {
        release_id: String,
        manifest_sha256: String,
    },
    Transition(ReleaseTransitionData),
    Pointer {
        revision: u64,
        release_id: String,
        previous_release_id: Option<String>,
    },
}

impl ReleaseBaselineMember {
    fn key(&self) -> String {
        match self {
            Self::Generation { release_id, .. } => format!("generation.{release_id}"),
            Self::Transition(data) => format!("transition.{}", data.transition_id()),
            Self::Pointer { revision, .. } => format!("pointer.{revision:020}"),
        }
    }

    fn has_fact(&self) -> bool {
        !matches!(self, Self::Pointer { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BaselineSummary {
    members: usize,
    members_sha256: String,
    active: Option<ReleaseBaselineMember>,
}

#[derive(Clone, PartialEq, Eq)]
struct ReleaseSnapshot {
    members: Vec<ReleaseBaselineMember>,
    active: Option<ReleaseBaselineMember>,
}

impl ReleaseSnapshot {
    fn contains(&self, member: &ReleaseBaselineMember) -> bool {
        self.members
            .binary_search_by_key(&member.key(), ReleaseBaselineMember::key)
            .is_ok_and(|index| self.members[index] == *member)
    }

    fn summary(&self) -> RuntimeStateResult<BaselineSummary> {
        Ok(BaselineSummary {
            members: self.members.len(),
            members_sha256: sha256(&encode(&self.members)?),
            active: self.active.clone(),
        })
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleaseSource {
    reference: ReleaseLedgerSourceReference,
    object_sha256: String,
}

pub enum ReleaseStateObservation {
    Applied,
    Unchanged,
    Unknown,
}

enum ReleaseChange {
    Freeze {
        snapshot: ReleaseSnapshot,
        payload: Vec<u8>,
        migration: StateMigrationData,
    },
    Stage(RuntimeReleaseSet),
    Transition {
        preview: ReleaseTransitionPreview,
        intent: Box<PersistedEvent>,
    },
    Legacy(ReleaseBaselineMember),
}

pub struct PreparedReleaseState {
    store: Arc<RuntimeStateStore>,
    change: ReleaseChange,
    before: String,
    applied_sequence: AtomicU64,
}

impl PreparedReleaseState {
    pub fn migration(&self) -> Option<&StateMigrationData> {
        match &self.change {
            ReleaseChange::Freeze { migration, .. } => Some(migration),
            _ => None,
        }
    }

    pub fn apply(
        &self,
        scope: &RuntimeTransaction<'_, '_>,
        event: &PersistedEvent,
    ) -> RuntimeStateResult<()> {
        self.require_owner(scope)?;
        verify_fact(&self.store.database, scope, event)?;
        if !matches!(&self.change, ReleaseChange::Freeze { .. }) {
            self.store.read_release_baseline(scope)?;
        }
        if self.store.release_fingerprint(scope.sql())? != self.before {
            return Err(request("release_state_changed", "apply_release_state"));
        }
        let (key, object) = match &self.change {
            ReleaseChange::Freeze {
                snapshot,
                payload,
                migration,
            } => {
                if !matches!(event.payload(), EventPayload::State(StatePayload::Migrated(value)) if value.migration() == migration)
                {
                    return Err(release_error("release_boundary_fact_mismatch"));
                }
                self.store.require_release_boundary_absent(scope.sql())?;
                let boundary =
                    actingcommand_ledger::read_release_baseline_source(&self.store.database, scope)
                        .map_err(release_source_error)?
                        .ok_or_else(|| release_error("release_boundary_source_missing"))?;
                if boundary.sequence() != event.sequence()
                    || boundary.event_id() != event.event_id()
                {
                    return Err(release_error("release_boundary_source_mismatch"));
                }
                if self.store.release_snapshot(scope.sql())? != *snapshot {
                    return Err(release_error("release_baseline_changed"));
                }
                let actual = self.store.migrate_document_in_transaction(
                    scope.sql(),
                    RELEASE_BASELINE_STATE_KEY,
                    FROM_SCHEMA,
                    TO_SCHEMA,
                    payload,
                )?;
                if actual != *migration {
                    return Err(release_error("release_boundary_migration_mismatch"));
                }
                for member in &snapshot.members {
                    let bytes = encode(member)?;
                    self.store.write_projection_in_transaction(
                        scope.sql(),
                        MEMBERS,
                        &member.key(),
                        event.sequence(),
                        &bytes,
                        &sha256(&bytes),
                    )?;
                }
                ("boundary".to_owned(), payload.clone())
            }
            ReleaseChange::Stage(manifest) => {
                self.store.require_release_ready(scope)?;
                if !matches!(event.payload(), EventPayload::Release(ReleasePayload::Staged(value)) if value.manifest() == manifest)
                {
                    return Err(release_error("release_stage_fact_mismatch"));
                }
                if query_release(scope.sql(), manifest.release_id())?.is_some() {
                    return Err(release_error("release_generation_identity_conflict"));
                }
                let bytes = encode(manifest)?;
                let digest = manifest.manifest_sha256();
                let tag = self.store.integrity_tag(
                    "release-generation-v1",
                    &[manifest.release_id().as_bytes(), digest.as_bytes(), &bytes],
                );
                scope.sql().execute("INSERT INTO release_generations (release_id,manifest_json,manifest_sha256,integrity_tag) VALUES (?1,?2,?3,?4)", params![manifest.release_id(), bytes, digest, tag])
                    .map_err(|_| release_error("release_generation_write_failed"))?;
                let member = generation_member(manifest);
                (member.key(), encode(&member)?)
            }
            ReleaseChange::Transition { preview, intent } => {
                self.store.require_release_ready(scope)?;
                verify_fact(&self.store.database, scope, intent)?;
                if intent.sequence() >= event.sequence()
                    || !matches!(intent.payload(), EventPayload::Release(ReleasePayload::TransitionIntent(value)) if value.transition() == &preview.data)
                {
                    return Err(release_error("release_intent_identity_mismatch"));
                }
                let target = match preview.data.kind() {
                    ReleaseTransitionKind::Activate => {
                        actingcommand_ledger::critical::ReleaseTransitionTarget::Activated
                    }
                    ReleaseTransitionKind::Rollback => {
                        actingcommand_ledger::critical::ReleaseTransitionTarget::RolledBack
                    }
                };
                actingcommand_ledger::critical::validate_release_transaction_outcome(
                    target, intent, event, true,
                )
                .map_err(|_| release_error("release_critical_outcome_mismatch"))?;
                let member = ReleaseBaselineMember::Transition(preview.data.clone());
                validate_member_fact(&member, event, false)?;
                self.store
                    .apply_release_transition_metadata(scope.sql(), preview)?;
                (member.key(), encode(&member)?)
            }
            ReleaseChange::Legacy(member) => {
                let baseline = self
                    .store
                    .read_release_baseline(scope)?
                    .ok_or_else(|| release_error("release_boundary_missing"))?;
                if !baseline.contains(member) {
                    return Err(release_error("release_legacy_member_unknown"));
                }
                validate_member_fact(member, event, true)?;
                if self
                    .store
                    .release_source(scope.sql(), &member.key())?
                    .is_some()
                {
                    return Err(release_error("release_source_already_bound"));
                }
                (member.key(), encode(member)?)
            }
        };
        self.store
            .bind_release_source(scope, &key, &object, event)?;
        match &self.change {
            ReleaseChange::Stage(_) | ReleaseChange::Transition { .. } => {
                self.store.require_release_ready(scope)?
            }
            ReleaseChange::Freeze { .. } | ReleaseChange::Legacy(_) => {
                self.store.read_release_baseline(scope)?;
            }
        }
        self.applied_sequence
            .store(event.sequence(), Ordering::Release);
        Ok(())
    }

    pub fn observe(
        &self,
        scope: &RuntimeTransaction<'_, '_>,
    ) -> RuntimeStateResult<ReleaseStateObservation> {
        self.require_owner(scope)?;
        self.store.read_release_baseline(scope)?;
        let (key, object) = match &self.change {
            ReleaseChange::Freeze { payload, .. } => ("boundary".to_owned(), payload.clone()),
            ReleaseChange::Stage(manifest) => {
                let member = generation_member(manifest);
                (member.key(), encode(&member)?)
            }
            ReleaseChange::Transition { preview, .. } => {
                let member = ReleaseBaselineMember::Transition(preview.data.clone());
                (member.key(), encode(&member)?)
            }
            ReleaseChange::Legacy(member) => (member.key(), encode(member)?),
        };
        let sequence = self.applied_sequence.load(Ordering::Acquire);
        if sequence != 0
            && self
                .store
                .verify_release_source(scope, &key, &object)?
                .is_some_and(|entry| entry.ledger_sequence() == sequence)
        {
            match &self.change {
                ReleaseChange::Freeze { snapshot, .. } => {
                    if self.store.read_release_baseline(scope)?.as_ref() != Some(snapshot) {
                        return Ok(ReleaseStateObservation::Unknown);
                    }
                }
                ReleaseChange::Transition { preview, .. } => {
                    let active = self
                        .store
                        .read_active_release_metadata(scope.sql(), "observe_release_state")?;
                    if active.as_ref().is_none_or(|active| {
                        active.revision() != preview.data.pointer_revision()
                            || active.manifest().release_id() != preview.data.release_id()
                    }) {
                        return Ok(ReleaseStateObservation::Unknown);
                    }
                    self.store.require_release_ready(scope)?;
                }
                ReleaseChange::Stage(_) => self.store.require_release_ready(scope)?,
                ReleaseChange::Legacy(_) => {
                    self.store.read_release_baseline(scope)?;
                }
            }
            return Ok(ReleaseStateObservation::Applied);
        }
        Ok(
            if self.store.release_fingerprint(scope.sql())? == self.before {
                ReleaseStateObservation::Unchanged
            } else {
                ReleaseStateObservation::Unknown
            },
        )
    }

    fn require_owner(&self, scope: &RuntimeTransaction<'_, '_>) -> RuntimeStateResult<()> {
        if !scope.belongs_to(&self.store.database) {
            return Err(release_error("state_transaction_owner_mismatch"));
        }
        Ok(())
    }
}

impl RuntimeStateStore {
    pub fn prepare_release_boundary(
        self: &Arc<Self>,
        fact: Option<&PersistedEvent>,
    ) -> RuntimeStateResult<Option<PreparedReleaseState>> {
        let mut connection = self.connection("prepare_release_boundary")?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(|_| release_error("state_transaction_begin_failed"))?;
        let scope = self.database.borrow_transaction(&transaction);
        let boundary = actingcommand_ledger::read_release_baseline_source(&self.database, &scope)
            .map_err(release_source_error)?;
        if self.read_release_baseline(&scope)?.is_some() {
            let fact = fact.ok_or_else(|| release_error("release_boundary_source_missing"))?;
            let boundary =
                boundary.ok_or_else(|| release_error("release_boundary_source_missing"))?;
            if boundary.sequence() != fact.sequence() || boundary.event_id() != fact.event_id() {
                return Err(release_error("release_boundary_source_mismatch"));
            }
            verify_fact(&self.database, &scope, fact)?;
            let document = self
                .read_boundary_document(&transaction)?
                .ok_or_else(|| release_error("release_boundary_missing"))?;
            let expected = boundary_migration(document.payload())?;
            if !matches!(fact.payload(), EventPayload::State(StatePayload::Migrated(value)) if value.migration() == &expected)
            {
                return Err(release_error("release_boundary_fact_mismatch"));
            }
            let source = self
                .verify_release_source(&scope, "boundary", document.payload())?
                .ok_or_else(|| release_error("release_boundary_source_missing"))?;
            if source.ledger_sequence() != fact.sequence() {
                return Err(release_error("release_boundary_source_mismatch"));
            }
            return Ok(None);
        }
        if fact.is_some() || boundary.is_some() {
            return Err(release_error("release_boundary_state_missing"));
        }
        self.require_release_boundary_absent(&transaction)?;
        let snapshot = self.release_snapshot(&transaction)?;
        let before = self.release_fingerprint(&transaction)?;
        let payload = encode(&snapshot.summary()?)?;
        validate_document_input(RELEASE_BASELINE_STATE_KEY, TO_SCHEMA, &payload)?;
        let migration = boundary_migration(&payload)?;
        drop(transaction);
        drop(connection);
        self.verify_snapshot_artifacts(&snapshot)?;
        Ok(Some(PreparedReleaseState {
            store: Arc::clone(self),
            change: ReleaseChange::Freeze {
                snapshot,
                payload,
                migration,
            },
            before,
            applied_sequence: AtomicU64::new(0),
        }))
    }

    pub fn prepare_release_stage(
        self: &Arc<Self>,
        manifest: &RuntimeReleaseSet,
        sources: &ReleaseArtifactSources,
    ) -> RuntimeStateResult<Option<PreparedReleaseState>> {
        manifest
            .validate()
            .map_err(|_| request("release_manifest_invalid", "stage_release"))?;
        let mut connection = self.connection("prepare_release_stage")?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(|_| release_error("state_transaction_begin_failed"))?;
        let scope = self.database.borrow_transaction(&transaction);
        self.require_release_ready(&scope)?;
        let existing = query_release(&transaction, manifest.release_id())?
            .map(|row| self.validate_release_metadata(row, "prepare_release_stage"))
            .transpose()?;
        if existing.as_ref().is_some_and(|value| value != manifest) {
            return Err(release_error("release_generation_identity_conflict"));
        }
        let before = self.release_fingerprint(&transaction)?;
        drop(transaction);
        drop(connection);
        self.publish_release_artifacts(manifest, sources)?;
        self.verify_release_artifacts(manifest, "prepare_release_stage")?;
        if existing.is_some() {
            return Ok(None);
        }
        Ok(Some(PreparedReleaseState {
            store: Arc::clone(self),
            change: ReleaseChange::Stage(manifest.clone()),
            before,
            applied_sequence: AtomicU64::new(0),
        }))
    }

    pub fn prepare_release_transition(
        self: &Arc<Self>,
        preview: &ReleaseTransitionPreview,
        intent: &PersistedEvent,
    ) -> RuntimeStateResult<PreparedReleaseState> {
        preview
            .data
            .validate()
            .map_err(|_| request("release_transition_invalid", "prepare_release_transition"))?;
        let mut connection = self.connection("prepare_release_transition")?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(|_| release_error("state_transaction_begin_failed"))?;
        let scope = self.database.borrow_transaction(&transaction);
        self.require_release_ready(&scope)?;
        let manifest = query_release(&transaction, preview.data.release_id())?
            .ok_or_else(|| request("release_generation_unknown", "prepare_release_transition"))?;
        let manifest = self.validate_release_metadata(manifest, "prepare_release_transition")?;
        let active =
            self.read_active_release_metadata(&transaction, "prepare_release_transition")?;
        let before = self.release_fingerprint(&transaction)?;
        drop(transaction);
        drop(connection);
        self.verify_release_artifacts(&manifest, "prepare_release_transition")?;
        if let Some(active) = active {
            self.verify_release_artifacts(active.manifest(), "prepare_release_transition")?;
        }
        Ok(PreparedReleaseState {
            store: Arc::clone(self),
            change: ReleaseChange::Transition {
                preview: preview.clone(),
                intent: Box::new(intent.clone()),
            },
            before,
            applied_sequence: AtomicU64::new(0),
        })
    }

    pub fn release_baseline_members(&self) -> RuntimeStateResult<Vec<ReleaseBaselineMember>> {
        let mut connection = self.connection("read_release_baseline")?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(|_| release_error("state_transaction_begin_failed"))?;
        let scope = self.database.borrow_transaction(&transaction);
        let baseline = self
            .read_release_baseline(&scope)?
            .ok_or_else(|| release_error("release_boundary_missing"))?;
        Ok(baseline
            .members
            .into_iter()
            .filter(ReleaseBaselineMember::has_fact)
            .collect())
    }

    pub fn release_generation_manifest(
        &self,
        release_id: &str,
    ) -> RuntimeStateResult<RuntimeReleaseSet> {
        let connection = self.connection("read_release_generation")?;
        let row = query_release(&connection, release_id)?
            .ok_or_else(|| release_error("release_generation_missing"))?;
        let manifest = self.validate_release_metadata(row, "read_release_generation")?;
        drop(connection);
        self.verify_release_artifacts(&manifest, "read_release_generation")?;
        Ok(manifest)
    }

    pub fn prepare_release_legacy_member(
        self: &Arc<Self>,
        member: &ReleaseBaselineMember,
        fact: Option<&PersistedEvent>,
    ) -> RuntimeStateResult<Option<PreparedReleaseState>> {
        let mut connection = self.connection("prepare_release_legacy_member")?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| release_error("state_transaction_begin_failed"))?;
        let scope = self.database.borrow_transaction(&transaction);
        let baseline = self
            .read_release_baseline(&scope)?
            .ok_or_else(|| release_error("release_boundary_missing"))?;
        if !member.has_fact() || !baseline.contains(member) {
            return Err(release_error("release_legacy_member_unknown"));
        }
        let object = encode(member)?;
        let source = self.verify_release_source(&scope, &member.key(), &object)?;
        if let Some(fact) = fact {
            validate_member_fact(member, fact, true)?;
            verify_fact(&self.database, &scope, fact)?;
            if let Some(source) = source {
                if source.ledger_sequence() != fact.sequence() {
                    return Err(release_error("release_source_identity_conflict"));
                }
            } else {
                self.bind_release_source(&scope, &member.key(), &object, fact)?;
            }
            transaction.commit().map_err(|error| {
                release_error("state_transaction_commit_failed").with_detail(error.to_string())
            })?;
            return Ok(None);
        }
        if source.is_some() {
            return Err(release_error("release_source_fact_missing"));
        }
        let before = self.release_fingerprint(&transaction)?;
        Ok(Some(PreparedReleaseState {
            store: Arc::clone(self),
            change: ReleaseChange::Legacy(member.clone()),
            before,
            applied_sequence: AtomicU64::new(0),
        }))
    }

    pub fn verify_release_atomic_ready(&self) -> RuntimeStateResult<()> {
        let mut connection = self.connection("verify_release_atomic_ready")?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(|_| release_error("state_transaction_begin_failed"))?;
        self.require_release_ready(&self.database.borrow_transaction(&transaction))
    }

    pub fn verify_release_committed_fact(&self, event: &PersistedEvent) -> RuntimeStateResult<()> {
        let mut connection = self.connection("verify_release_fact")?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(|_| release_error("state_transaction_begin_failed"))?;
        let scope = self.database.borrow_transaction(&transaction);
        verify_fact(&self.database, &scope, event)?;
        let baseline = self
            .read_release_baseline(&scope)?
            .ok_or_else(|| release_error("release_boundary_missing"))?;
        let member = match event.payload() {
            EventPayload::Release(ReleasePayload::Staged(value)) => {
                let row = query_release(&transaction, value.manifest().release_id())?
                    .ok_or_else(|| release_error("release_generation_missing"))?;
                let manifest = self.validate_release_metadata(row, "verify_release_fact")?;
                if &manifest != value.manifest() {
                    return Err(release_error("release_source_payload_mismatch"));
                }
                generation_member(&manifest)
            }
            EventPayload::Release(
                ReleasePayload::Activated(value) | ReleasePayload::RolledBack(value),
            ) => {
                let row =
                    query_release_transition(&transaction, value.transition().transition_id())?
                        .ok_or_else(|| release_error("release_transition_missing"))?;
                ReleaseBaselineMember::Transition(
                    self.validate_release_transition_row(&row, "verify_release_fact")?,
                )
            }
            _ => return Err(release_error("release_source_payload_mismatch")),
        };
        validate_member_fact(&member, event, baseline.contains(&member))?;
        let source = self
            .verify_release_source(&scope, &member.key(), &encode(&member)?)?
            .ok_or_else(|| release_error("release_source_unclosed"))?;
        if source.ledger_sequence() != event.sequence() {
            return Err(release_error("release_source_identity_conflict"));
        }
        Ok(())
    }

    pub(super) fn require_legacy_release_writer(&self) -> RuntimeStateResult<()> {
        let mut connection = self.connection("authorize_release_write")?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(|_| release_error("state_transaction_begin_failed"))?;
        self.require_legacy_release_connection(&self.database.borrow_transaction(&transaction))
    }

    pub(super) fn require_legacy_release_connection(
        &self,
        scope: &RuntimeTransaction<'_, '_>,
    ) -> RuntimeStateResult<()> {
        let connection = scope.sql();
        self.require_release_boundary_absent(connection)?;
        if self.release_boundary_fact_exists(scope)? {
            return Err(request(
                "release_transaction_owner_required",
                "authorize_release_write",
            ));
        }
        Ok(())
    }

    pub(super) fn verify_release_read_sources(
        &self,
        scope: &RuntimeTransaction<'_, '_>,
    ) -> RuntimeStateResult<()> {
        let connection = scope.sql();
        if self.read_boundary_document(connection)?.is_some() {
            self.require_release_ready(scope)
        } else {
            self.require_legacy_release_connection(scope)
        }
    }

    fn require_release_ready(&self, scope: &RuntimeTransaction<'_, '_>) -> RuntimeStateResult<()> {
        let connection = scope.sql();
        self.read_release_baseline(scope)?
            .ok_or_else(|| release_error("release_boundary_missing"))?;
        let current = self.release_snapshot(connection)?;
        for member in &current.members {
            if member.has_fact()
                && self
                    .verify_release_source(scope, &member.key(), &encode(member)?)?
                    .is_none()
            {
                return Err(release_error("release_source_unclosed"));
            }
        }
        let source_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM projection_entries WHERE namespace=?1",
                [SOURCES],
                |row| row.get(0),
            )
            .map_err(|_| release_error("release_source_query_failed"))?;
        let expected = current
            .members
            .iter()
            .filter(|member| member.has_fact())
            .count()
            + 1;
        if source_count != expected as i64 {
            return Err(release_error("release_source_set_mismatch"));
        }
        Ok(())
    }

    fn read_boundary_document(
        &self,
        connection: &Connection,
    ) -> RuntimeStateResult<Option<StateDocument>> {
        query_document(connection, RELEASE_BASELINE_STATE_KEY)?
            .map(|row| self.validate_document_row(row, "read_release_boundary"))
            .transpose()
    }

    fn require_release_boundary_absent(&self, connection: &Connection) -> RuntimeStateResult<()> {
        let count: i64 = connection.query_row("SELECT (SELECT COUNT(*) FROM state_documents WHERE state_key=?1) + (SELECT COUNT(*) FROM state_document_history WHERE state_key=?1) + (SELECT COUNT(*) FROM state_migrations WHERE json_extract(data_json,'$.state_key')=?1) + (SELECT COUNT(*) FROM projection_entries WHERE namespace IN (?2,?3))", params![RELEASE_BASELINE_STATE_KEY, MEMBERS, SOURCES], |row| row.get(0)).map_err(|_| release_error("release_boundary_query_failed"))?;
        if count != 0 {
            return Err(release_error("release_boundary_already_present"));
        }
        Ok(())
    }

    fn release_boundary_fact_exists(
        &self,
        scope: &RuntimeTransaction<'_, '_>,
    ) -> RuntimeStateResult<bool> {
        actingcommand_ledger::read_release_baseline_source(&self.database, scope)
            .map(|source| source.is_some())
            .map_err(release_source_error)
    }

    fn release_source(
        &self,
        connection: &Connection,
        key: &str,
    ) -> RuntimeStateResult<Option<ProjectionEntry>> {
        query_projection_entry(connection, SOURCES, key)?
            .map(|row| self.validate_projection_row(row, "read_release_source"))
            .transpose()
    }

    fn bind_release_source(
        &self,
        scope: &RuntimeTransaction<'_, '_>,
        key: &str,
        object: &[u8],
        event: &PersistedEvent,
    ) -> RuntimeStateResult<()> {
        verify_fact(&self.database, scope, event)?;
        let reference =
            actingcommand_ledger::capture_release_source_reference(&self.database, scope, event)
                .map_err(release_source_error)?;
        let source = ReleaseSource {
            reference,
            object_sha256: sha256(object),
        };
        let payload = encode(&source)?;
        if self.release_source(scope.sql(), key)?.is_some() {
            return Err(release_error("release_source_identity_conflict"));
        }
        self.write_projection_in_transaction(
            scope.sql(),
            SOURCES,
            key,
            event.sequence(),
            &payload,
            &sha256(&payload),
        )?;
        Ok(())
    }

    fn verify_release_source(
        &self,
        scope: &RuntimeTransaction<'_, '_>,
        key: &str,
        object: &[u8],
    ) -> RuntimeStateResult<Option<ProjectionEntry>> {
        let connection = scope.sql();
        let Some(entry) = self.release_source(connection, key)? else {
            return Ok(None);
        };
        let source: ReleaseSource = decode(entry.payload())?;
        let verified = actingcommand_ledger::verify_release_source_reference(
            &self.database,
            scope,
            &source.reference,
        )
        .map_err(release_source_error)?;
        if source.object_sha256 != sha256(object)
            || verified.sequence() != entry.ledger_sequence()
            || verified.origin().source() != EventSource::Runtime
            || verified.origin().module() != OriginModule::Runtime
            || verified.origin().actor() != EventActor::Runtime
        {
            return Err(release_error("release_source_fact_mismatch"));
        }
        if key == "boundary" {
            let expected = boundary_migration(object)?;
            if !matches!(verified.payload(), EventPayload::State(StatePayload::Migrated(value)) if value.migration() == &expected)
            {
                return Err(release_error("release_boundary_source_mismatch"));
            }
        } else {
            let member: ReleaseBaselineMember = decode(object)?;
            let legacy = query_projection_entry(connection, MEMBERS, key)?
                .map(|row| self.validate_projection_row(row, "verify_release_source"))
                .transpose()?
                .is_some_and(|entry| entry.payload() == object);
            if member.key() != key {
                return Err(release_error("release_source_identity_conflict"));
            }
            validate_member_payload(&member, verified.payload(), legacy)?;
        }
        Ok(Some(entry))
    }

    fn release_snapshot(&self, connection: &Connection) -> RuntimeStateResult<ReleaseSnapshot> {
        let count: i64 = connection.query_row("SELECT (SELECT COUNT(*) FROM release_generations) + (SELECT COUNT(*) FROM release_transitions) + (SELECT COUNT(*) FROM release_pointer_history)", [], |row| row.get(0)).map_err(|_| release_error("release_baseline_query_failed"))?;
        if count < 0 || count > MAX_MEMBERS as i64 {
            return Err(release_error("release_baseline_member_limit"));
        }
        let oversized: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM release_generations WHERE length(manifest_json)>?1) OR EXISTS(SELECT 1 FROM release_transitions WHERE length(data_json)>?2)", params![MAX_STATE_DOCUMENT_BYTES as i64, MAX_PROJECTION_ENTRY_BYTES as i64], |row| row.get(0)).map_err(|_| release_error("release_baseline_query_failed"))?;
        if oversized {
            return Err(release_error("release_baseline_member_size_invalid"));
        }
        let mut members = Vec::with_capacity(count as usize);
        let mut generations = BTreeMap::new();
        let mut statement = connection.prepare("SELECT release_id,manifest_json,manifest_sha256,integrity_tag FROM release_generations ORDER BY release_id").map_err(|_| release_error("release_generation_query_failed"))?;
        let rows = statement
            .query_map([], map_release_row)
            .map_err(|_| release_error("release_generation_query_failed"))?;
        for row in rows {
            let manifest = self.validate_release_metadata(
                row.map_err(|_| release_error("release_generation_read_failed"))?,
                "release_baseline",
            )?;
            generations.insert(manifest.release_id().to_owned(), manifest.manifest_sha256());
            members.push(generation_member(&manifest));
        }
        let mut transitions = BTreeMap::new();
        let mut statement = connection.prepare("SELECT transition_id,data_json,integrity_tag FROM release_transitions ORDER BY transition_id").map_err(|_| release_error("release_transition_query_failed"))?;
        let rows = statement
            .query_map([], |row| {
                Ok(TransitionRow {
                    transition_id: row.get(0)?,
                    data_json: row.get(1)?,
                    integrity_tag: row.get(2)?,
                })
            })
            .map_err(|_| release_error("release_transition_query_failed"))?;
        for row in rows {
            let data = self.validate_release_transition_row(
                &row.map_err(|_| release_error("release_transition_read_failed"))?,
                "release_baseline",
            )?;
            if generations.get(data.release_id()).map(String::as_str)
                != Some(data.manifest_sha256())
                || data.transition_id()
                    != release_transition_id(
                        data.kind(),
                        data.previous_release_id(),
                        data.release_id(),
                        data.pointer_revision(),
                        data.manifest_sha256(),
                    )
                || data.validation_result() != StateValidationResult::Passed
                || data.recovery_action() != StateRecoveryAction::None
                || transitions
                    .insert(data.pointer_revision(), data.clone())
                    .is_some()
            {
                return Err(release_error("release_transition_history_conflict"));
            }
            members.push(ReleaseBaselineMember::Transition(data));
        }
        let mut statement = connection.prepare("SELECT revision,release_id,previous_release_id,integrity_tag FROM release_pointer_history ORDER BY revision").map_err(|_| release_error("release_history_query_failed"))?;
        let rows = statement
            .query_map([], |row| {
                Ok(PointerRow {
                    revision: row_u64(row, 0)?,
                    release_id: row.get(1)?,
                    previous_release_id: row.get(2)?,
                    integrity_tag: row.get(3)?,
                })
            })
            .map_err(|_| release_error("release_history_query_failed"))?;
        let mut previous: Option<String> = None;
        let mut active = None;
        let mut next_revision = 1_u64;
        let mut seen = BTreeSet::new();
        for row in rows {
            let pointer = row.map_err(|_| release_error("release_history_read_failed"))?;
            self.validate_pointer_row(&pointer, "release_baseline")?;
            let transition = transitions
                .remove(&pointer.revision)
                .ok_or_else(|| release_error("release_history_transition_missing"))?;
            if pointer.revision != next_revision
                || pointer.previous_release_id != previous
                || pointer.release_id == previous.as_deref().unwrap_or("")
                || transition.release_id() != pointer.release_id
                || transition.previous_release_id() != pointer.previous_release_id.as_deref()
                || (transition.kind() == ReleaseTransitionKind::Rollback
                    && !seen.contains(&pointer.release_id))
            {
                return Err(release_error("release_pointer_history_conflict"));
            }
            next_revision = next_revision
                .checked_add(1)
                .ok_or_else(|| release_error("release_pointer_revision_overflow"))?;
            previous = Some(pointer.release_id.clone());
            seen.insert(pointer.release_id.clone());
            let member = ReleaseBaselineMember::Pointer {
                revision: pointer.revision,
                release_id: pointer.release_id,
                previous_release_id: pointer.previous_release_id,
            };
            active = Some(member.clone());
            members.push(member);
        }
        if !transitions.is_empty() {
            return Err(release_error("release_transition_pointer_missing"));
        }
        let current = query_pointer(connection)?
            .map(|pointer| {
                self.validate_pointer_row(&pointer, "release_baseline")?;
                Ok::<_, RuntimeStateError>(ReleaseBaselineMember::Pointer {
                    revision: pointer.revision,
                    release_id: pointer.release_id,
                    previous_release_id: pointer.previous_release_id,
                })
            })
            .transpose()?;
        if current != active {
            return Err(release_error("release_active_history_mismatch"));
        }
        members.sort_by_key(ReleaseBaselineMember::key);
        for member in &members {
            validate_projection_input(MEMBERS, &member.key(), 1, &encode(member)?)?;
        }
        Ok(ReleaseSnapshot { members, active })
    }

    fn read_release_baseline(
        &self,
        scope: &RuntimeTransaction<'_, '_>,
    ) -> RuntimeStateResult<Option<ReleaseSnapshot>> {
        let connection = scope.sql();
        let Some(document) = self.read_boundary_document(connection)? else {
            self.require_release_boundary_absent(connection)?;
            if self.release_boundary_fact_exists(scope)? {
                return Err(release_error("release_boundary_state_missing"));
            }
            return Ok(None);
        };
        let history_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM state_document_history WHERE state_key=?1",
                [RELEASE_BASELINE_STATE_KEY],
                |row| row.get(0),
            )
            .map_err(|_| release_error("release_boundary_query_failed"))?;
        let history = query_document_revision(connection, RELEASE_BASELINE_STATE_KEY, 1)?
            .map(|row| self.validate_document_row(row, "release_boundary"))
            .transpose()?;
        if document.schema_version() != TO_SCHEMA
            || document.revision() != 1
            || document.previous_payload_sha256().is_some()
            || history_count != 1
            || history.as_ref() != Some(&document)
        {
            return Err(release_error("release_boundary_history_conflict"));
        }
        let expected = boundary_migration(document.payload())?;
        let migration = query_migration(connection, expected.migration_id())?
            .ok_or_else(|| release_error("release_boundary_migration_missing"))?;
        let migrations: i64 = connection.query_row("SELECT COUNT(*) FROM state_migrations WHERE json_extract(data_json,'$.state_key')=?1", [RELEASE_BASELINE_STATE_KEY], |row| row.get(0)).map_err(|_| release_error("release_boundary_query_failed"))?;
        if migrations != 1
            || self
                .validate_migration_row(&migration, "release_boundary")?
                .data
                != expected
        {
            return Err(release_error("release_boundary_migration_conflict"));
        }
        let source = self
            .verify_release_source(scope, "boundary", document.payload())?
            .ok_or_else(|| release_error("release_boundary_source_missing"))?;
        let summary: BaselineSummary = decode(document.payload())?;
        if summary.members > MAX_MEMBERS {
            return Err(release_error("release_baseline_member_limit"));
        }
        let entries = self.release_projection_entries(connection, MEMBERS)?;
        if entries.len() != summary.members {
            return Err(release_error("release_baseline_members_missing"));
        }
        let mut members = Vec::with_capacity(entries.len());
        let current = self.release_snapshot(connection)?;
        for entry in entries {
            let member: ReleaseBaselineMember = decode(entry.payload())?;
            if entry.entry_key() != member.key()
                || entry.ledger_sequence() != source.ledger_sequence()
                || !current.contains(&member)
            {
                return Err(release_error("release_baseline_member_conflict"));
            }
            members.push(member);
        }
        let snapshot = ReleaseSnapshot {
            members,
            active: summary.active.clone(),
        };
        if snapshot.summary()? != summary {
            return Err(release_error("release_baseline_digest_mismatch"));
        }
        let latest = snapshot
            .members
            .iter()
            .filter_map(|member| match member {
                ReleaseBaselineMember::Pointer { revision, .. } => Some((*revision, member)),
                _ => None,
            })
            .max_by_key(|(revision, _)| *revision)
            .map(|(_, member)| member.clone());
        if latest != snapshot.active {
            return Err(release_error("release_baseline_active_mismatch"));
        }
        Ok(Some(snapshot))
    }

    fn release_projection_entries(
        &self,
        connection: &Connection,
        namespace: &str,
    ) -> RuntimeStateResult<Vec<ProjectionEntry>> {
        let (count, largest): (i64, i64) = connection.query_row("SELECT COUNT(*),COALESCE(MAX(length(payload)),0) FROM projection_entries WHERE namespace=?1", [namespace], |row| Ok((row.get(0)?, row.get(1)?))).map_err(|_| release_error("release_projection_query_failed"))?;
        if count < 0
            || count > (MAX_MEMBERS + 1) as i64
            || largest > MAX_PROJECTION_ENTRY_BYTES as i64
        {
            return Err(release_error("release_projection_size_invalid"));
        }
        let mut statement = connection.prepare("SELECT namespace,entry_key,ledger_sequence,payload,payload_sha256,integrity_tag FROM projection_entries WHERE namespace=?1 ORDER BY entry_key").map_err(|_| release_error("release_projection_query_failed"))?;
        let rows = statement
            .query_map([namespace], |row| {
                Ok(ProjectionRow {
                    namespace: row.get(0)?,
                    entry_key: row.get(1)?,
                    ledger_sequence: row_u64(row, 2)?,
                    payload: row.get(3)?,
                    payload_sha256: row.get(4)?,
                    integrity_tag: row.get(5)?,
                })
            })
            .map_err(|_| release_error("release_projection_query_failed"))?;
        rows.map(|row| {
            self.validate_projection_row(
                row.map_err(|_| release_error("release_projection_read_failed"))?,
                "release_state",
            )
        })
        .collect()
    }

    fn release_fingerprint(&self, connection: &Connection) -> RuntimeStateResult<String> {
        let snapshot = self.release_snapshot(connection)?;
        let mut digest = Sha256::new();
        digest.update(encode(&snapshot.summary()?)?);
        if let Some(document) = self.read_boundary_document(connection)? {
            digest.update(document.payload_sha256().as_bytes());
            digest.update(document.revision().to_be_bytes());
            digest.update(document.schema_version().as_bytes());
        }
        for namespace in [MEMBERS, SOURCES] {
            for entry in self.release_projection_entries(connection, namespace)? {
                digest.update(encode(&(
                    namespace,
                    entry.entry_key(),
                    entry.ledger_sequence(),
                    entry.payload_sha256(),
                ))?);
            }
        }
        Ok(format!("sha256:{:x}", digest.finalize()))
    }

    fn verify_snapshot_artifacts(&self, snapshot: &ReleaseSnapshot) -> RuntimeStateResult<()> {
        for member in &snapshot.members {
            if let ReleaseBaselineMember::Generation {
                release_id,
                manifest_sha256,
            } = member
            {
                let manifest = self.release_generation_manifest(release_id)?;
                if manifest.manifest_sha256() != *manifest_sha256 {
                    return Err(release_error("release_baseline_changed"));
                }
            }
        }
        Ok(())
    }
}

pub(super) fn reject_release_key(key: &str) -> RuntimeStateResult<()> {
    if key == RELEASE_BASELINE_STATE_KEY {
        return Err(request(
            "release_state_owner_required",
            "write_state_document",
        ));
    }
    Ok(())
}

pub(super) fn reject_release_namespace(namespace: &str) -> RuntimeStateResult<()> {
    if matches!(namespace, MEMBERS | SOURCES) {
        return Err(request(
            "release_state_owner_required",
            "write_projection_entry",
        ));
    }
    Ok(())
}

fn generation_member(manifest: &RuntimeReleaseSet) -> ReleaseBaselineMember {
    ReleaseBaselineMember::Generation {
        release_id: manifest.release_id().to_owned(),
        manifest_sha256: manifest.manifest_sha256(),
    }
}

fn boundary_migration(payload: &[u8]) -> RuntimeStateResult<StateMigrationData> {
    let digest = sha256(payload);
    StateMigrationData::new(
        migration_id(RELEASE_BASELINE_STATE_KEY, FROM_SCHEMA, TO_SCHEMA, &digest),
        RELEASE_BASELINE_STATE_KEY,
        FROM_SCHEMA,
        TO_SCHEMA,
        digest,
        StateValidationResult::Passed,
        StateRecoveryAction::ImportedLegacy,
    )
    .map_err(|_| release_error("release_boundary_migration_invalid"))
}

fn verify_fact(
    database: &RuntimeDatabase,
    scope: &RuntimeTransaction<'_, '_>,
    event: &PersistedEvent,
) -> RuntimeStateResult<()> {
    if event.origin().source() != EventSource::Runtime
        || event.origin().module() != OriginModule::Runtime
        || event.origin().actor() != EventActor::Runtime
    {
        return Err(release_error("release_source_origin_invalid"));
    }
    actingcommand_ledger::verify_transaction_event(database, scope, event)
        .map_err(release_source_error)
}

fn validate_member_fact(
    member: &ReleaseBaselineMember,
    event: &PersistedEvent,
    legacy: bool,
) -> RuntimeStateResult<()> {
    validate_member_payload(member, event.payload(), legacy)
}

fn validate_member_payload(
    member: &ReleaseBaselineMember,
    payload: &EventPayload,
    legacy: bool,
) -> RuntimeStateResult<()> {
    if payload.effect_disposition() != Some(actingcommand_contract::EffectDisposition::Performed) {
        return Err(release_error("release_source_effect_mismatch"));
    }
    let matches = match (member, payload) {
        (
            ReleaseBaselineMember::Generation {
                release_id,
                manifest_sha256,
            },
            EventPayload::Release(ReleasePayload::Staged(value)),
        ) => {
            value.manifest().release_id() == release_id
                && value.manifest().manifest_sha256() == *manifest_sha256
        }
        (
            ReleaseBaselineMember::Transition(expected),
            EventPayload::Release(ReleasePayload::Activated(value)),
        ) if expected.kind() == ReleaseTransitionKind::Activate => {
            value.transition() == expected
                || (legacy && value.transition() == &expected.recovered_for_ledger())
        }
        (
            ReleaseBaselineMember::Transition(expected),
            EventPayload::Release(ReleasePayload::RolledBack(value)),
        ) if expected.kind() == ReleaseTransitionKind::Rollback => {
            value.transition() == expected
                || (legacy && value.transition() == &expected.recovered_for_ledger())
        }
        _ => false,
    };
    if !matches {
        return Err(release_error("release_source_payload_mismatch"));
    }
    Ok(())
}

fn encode(value: &impl Serialize) -> RuntimeStateResult<Vec<u8>> {
    serde_json::to_vec(value).map_err(|_| release_error("release_state_encode_failed"))
}
fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> RuntimeStateResult<T> {
    serde_json::from_slice(bytes).map_err(|_| release_error("release_state_decode_failed"))
}
fn release_error(code: &'static str) -> RuntimeStateError {
    fatal(code, "release_state")
}

fn release_source_error(error: actingcommand_ledger::GlobalLedgerError) -> RuntimeStateError {
    fatal(error.code(), error.operation())
        .with_detail(format!("{error}; detail={:?}", error.detail()))
}
