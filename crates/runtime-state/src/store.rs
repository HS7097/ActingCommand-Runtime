// SPDX-License-Identifier: AGPL-3.0-only

use crate::{RuntimeStateError, RuntimeStateResult};
use actingcommand_contract::{
    ReleaseTransitionData, ReleaseTransitionKind, RuntimeReleaseSet, StateMigrationData,
    StateRecoveryAction, StateTransitionStatus, StateValidationResult,
};
use rusqlite::{
    Connection, OptionalExtension, Transaction, TransactionBehavior, params, types::Type,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

pub const RUNTIME_STATE_SCHEMA_VERSION: &str = "actingcommand.runtime-state.v1";
pub const RUNTIME_STATE_DATABASE_FILE: &str = "runtime-state.sqlite";
pub const RUNTIME_STATE_INTEGRITY_KEY_FILE: &str = "runtime-state.key";
pub const RUNTIME_RELEASE_BLOB_DIRECTORY: &str = "release-blobs";

const MAX_STATE_DOCUMENT_BYTES: usize = 1024 * 1024;
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
static RELEASE_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateDocument {
    state_key: String,
    schema_version: String,
    revision: u64,
    payload: Vec<u8>,
    payload_sha256: String,
    previous_payload_sha256: Option<String>,
}

impl StateDocument {
    pub fn state_key(&self) -> &str {
        &self.state_key
    }

    pub fn schema_version(&self) -> &str {
        &self.schema_version
    }

    pub const fn revision(&self) -> u64 {
        self.revision
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    pub fn payload_sha256(&self) -> &str {
        &self.payload_sha256
    }

    pub fn previous_payload_sha256(&self) -> Option<&str> {
        self.previous_payload_sha256.as_deref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedRelease {
    manifest: RuntimeReleaseSet,
    created: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseArtifactSources {
    runtime: PathBuf,
    ui: PathBuf,
    resources: BTreeMap<String, PathBuf>,
}

impl ReleaseArtifactSources {
    pub fn new(
        runtime: impl Into<PathBuf>,
        ui: impl Into<PathBuf>,
        resources: BTreeMap<String, PathBuf>,
    ) -> Self {
        Self {
            runtime: runtime.into(),
            ui: ui.into(),
            resources,
        }
    }

    pub fn runtime(&self) -> &Path {
        &self.runtime
    }

    pub fn ui(&self) -> &Path {
        &self.ui
    }

    pub fn resources(&self) -> &BTreeMap<String, PathBuf> {
        &self.resources
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReleaseArtifactKey {
    Runtime,
    Ui,
    Resource(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedReleaseArtifact {
    release_id: String,
    content_sha256: String,
    path: PathBuf,
}

impl ResolvedReleaseArtifact {
    pub fn release_id(&self) -> &str {
        &self.release_id
    }

    pub fn content_sha256(&self) -> &str {
        &self.content_sha256
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl StagedRelease {
    pub const fn manifest(&self) -> &RuntimeReleaseSet {
        &self.manifest
    }

    pub const fn created(&self) -> bool {
        self.created
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveRelease {
    revision: u64,
    manifest: RuntimeReleaseSet,
    previous_release_id: Option<String>,
}

impl ActiveRelease {
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    pub const fn manifest(&self) -> &RuntimeReleaseSet {
        &self.manifest
    }

    pub fn previous_release_id(&self) -> Option<&str> {
        self.previous_release_id.as_deref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseTransitionPreview {
    data: ReleaseTransitionData,
}

impl ReleaseTransitionPreview {
    pub const fn data(&self) -> &ReleaseTransitionData {
        &self.data
    }
}

/// SQLite is the sole mutable-state authority. The keyed envelope detects out-of-band edits;
/// it is an integrity boundary, not encryption or DRM.
pub struct RuntimeStateStore {
    database_path: PathBuf,
    release_blobs: PathBuf,
    connection: Mutex<Connection>,
    integrity_key: Box<[u8]>,
}

impl RuntimeStateStore {
    pub fn open(root: &Path, integrity_key: &[u8]) -> RuntimeStateResult<Self> {
        if integrity_key.len() < 16 || integrity_key.len() > 1024 {
            return Err(fatal("state_integrity_key_invalid", "open_runtime_state"));
        }
        fs::create_dir_all(root)
            .map_err(|_| fatal("state_root_create_failed", "open_runtime_state"))?;
        require_regular_directory(root)?;
        let release_blobs = prepare_release_blob_store(root)?;
        let database_path = root.join(RUNTIME_STATE_DATABASE_FILE);
        let database_existed = database_path.exists();
        if database_existed {
            require_regular_file(&database_path)?;
        }
        let integrity_key = load_or_create_integrity_key(root, integrity_key, database_existed)?;
        let connection = Connection::open(&database_path)
            .map_err(|_| fatal("state_database_open_failed", "open_runtime_state"))?;
        connection
            .busy_timeout(BUSY_TIMEOUT)
            .map_err(|_| fatal("state_database_config_failed", "open_runtime_state"))?;
        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL;",
            )
            .map_err(|_| fatal("state_database_config_failed", "open_runtime_state"))?;
        connection
            .execute_batch(include_str!("schema.sql"))
            .map_err(|_| fatal("state_schema_initialize_failed", "open_runtime_state"))?;
        let schema_version = connection
            .query_row(
                "SELECT value FROM state_meta WHERE key = 'schema_version'",
                [],
                |row| row.get::<_, String>(0),
            )
            .map_err(|_| fatal("state_schema_metadata_missing", "open_runtime_state"))?;
        if schema_version != RUNTIME_STATE_SCHEMA_VERSION {
            return Err(fatal(
                "state_schema_version_unsupported",
                "open_runtime_state",
            ));
        }
        let integrity = connection
            .query_row("PRAGMA quick_check(1)", [], |row| row.get::<_, String>(0))
            .map_err(|_| fatal("state_integrity_check_failed", "open_runtime_state"))?;
        if integrity != "ok" {
            return Err(fatal("state_database_corrupt", "open_runtime_state"));
        }
        let store = Self {
            database_path,
            release_blobs,
            connection: Mutex::new(connection),
            integrity_key,
        };
        store.validate_all()?;
        Ok(store)
    }

    pub fn database_path(&self) -> &Path {
        &self.database_path
    }

    pub fn read_json_document(&self, state_key: &str) -> RuntimeStateResult<Option<StateDocument>> {
        validate_state_key(state_key)?;
        let connection = self.connection("read_state_document")?;
        let row = query_document(&connection, state_key)?;
        row.map(|row| self.validate_document_row(row, "read_state_document"))
            .transpose()
    }

    pub fn write_json_document(
        &self,
        state_key: &str,
        schema_version: &str,
        payload: &[u8],
        expected_payload_sha256: Option<&str>,
    ) -> RuntimeStateResult<StateDocument> {
        validate_document_input(state_key, schema_version, payload)?;
        let payload_sha256 = sha256(payload);
        let mut connection = self.connection("write_state_document")?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| fatal("state_transaction_begin_failed", "write_state_document"))?;
        let current = query_document(&transaction, state_key)?;
        if current.as_ref().map(|row| row.payload_sha256.as_str()) != expected_payload_sha256 {
            return Err(request("state_document_changed", "write_state_document"));
        }
        if let Some(current) = current.as_ref().filter(|row| {
            row.schema_version == schema_version && row.payload_sha256 == payload_sha256
        }) {
            let current = DocumentRow {
                state_key: current.state_key.clone(),
                schema_version: current.schema_version.clone(),
                revision: current.revision,
                payload: current.payload.clone(),
                payload_sha256: current.payload_sha256.clone(),
                previous_payload_sha256: current.previous_payload_sha256.clone(),
                integrity_tag: current.integrity_tag.clone(),
            };
            transaction
                .commit()
                .map_err(|_| fatal("state_transaction_commit_failed", "write_state_document"))?;
            return self.validate_document_row(current, "write_state_document");
        }
        let revision = current
            .as_ref()
            .map_or(Ok(1_u64), |row| row.revision.checked_add(1).ok_or(()))
            .map_err(|()| fatal("state_revision_overflow", "write_state_document"))?;
        let previous = current.map(|row| row.payload_sha256);
        let integrity_tag = self.document_integrity_tag(
            state_key,
            schema_version,
            revision,
            payload,
            &payload_sha256,
            previous.as_deref(),
        );
        insert_document_revision(
            &transaction,
            state_key,
            schema_version,
            revision,
            payload,
            &payload_sha256,
            previous.as_deref(),
            &integrity_tag,
        )?;
        transaction
            .commit()
            .map_err(|_| fatal("state_transaction_commit_failed", "write_state_document"))?;
        Ok(StateDocument {
            state_key: state_key.to_owned(),
            schema_version: schema_version.to_owned(),
            revision,
            payload: payload.to_vec(),
            payload_sha256,
            previous_payload_sha256: previous,
        })
    }

    pub fn migrate_legacy_json_document(
        &self,
        state_key: &str,
        from_schema_version: &str,
        to_schema_version: &str,
        payload: &[u8],
    ) -> RuntimeStateResult<StateMigrationData> {
        validate_document_input(state_key, to_schema_version, payload)?;
        validate_version(from_schema_version, "migrate_state_document")?;
        if from_schema_version == to_schema_version {
            return Err(request(
                "state_migration_schema_unchanged",
                "migrate_state_document",
            ));
        }
        let payload_sha256 = sha256(payload);
        let migration_id = migration_id(
            state_key,
            from_schema_version,
            to_schema_version,
            &payload_sha256,
        );
        let data = StateMigrationData::new(
            migration_id.clone(),
            state_key,
            from_schema_version,
            to_schema_version,
            payload_sha256.clone(),
            StateValidationResult::Passed,
            StateRecoveryAction::ImportedLegacy,
        )
        .map_err(|_| request("state_migration_invalid", "migrate_state_document"))?;
        let data_json = serde_json::to_vec(&data)
            .map_err(|_| fatal("state_migration_encode_failed", "migrate_state_document"))?;
        let mut connection = self.connection("migrate_state_document")?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| fatal("state_transaction_begin_failed", "migrate_state_document"))?;
        let current = query_document(&transaction, state_key)?;
        let existing_migration = query_migration(&transaction, data.migration_id())?;
        match current {
            Some(current) if current.payload_sha256 != payload_sha256 => {
                return Err(fatal(
                    "state_migration_content_conflict",
                    "migrate_state_document",
                ));
            }
            None => {
                if existing_migration.is_some() {
                    return Err(fatal(
                        "state_migration_state_missing",
                        "migrate_state_document",
                    ));
                }
                let integrity_tag = self.document_integrity_tag(
                    state_key,
                    to_schema_version,
                    1,
                    payload,
                    &payload_sha256,
                    None,
                );
                insert_document_revision(
                    &transaction,
                    state_key,
                    to_schema_version,
                    1,
                    payload,
                    &payload_sha256,
                    None,
                    &integrity_tag,
                )?;
            }
            Some(current) if current.schema_version == from_schema_version => {
                if existing_migration.is_some() {
                    return Err(fatal(
                        "state_migration_state_conflict",
                        "migrate_state_document",
                    ));
                }
                let revision = current
                    .revision
                    .checked_add(1)
                    .ok_or_else(|| fatal("state_revision_overflow", "migrate_state_document"))?;
                let integrity_tag = self.document_integrity_tag(
                    state_key,
                    to_schema_version,
                    revision,
                    payload,
                    &payload_sha256,
                    Some(&current.payload_sha256),
                );
                insert_document_revision(
                    &transaction,
                    state_key,
                    to_schema_version,
                    revision,
                    payload,
                    &payload_sha256,
                    Some(&current.payload_sha256),
                    &integrity_tag,
                )?;
            }
            Some(current) if current.schema_version == to_schema_version => {
                let existing = existing_migration.as_ref().ok_or_else(|| {
                    fatal("state_migration_record_missing", "migrate_state_document")
                })?;
                let existing = self.validate_migration_row(existing, "migrate_state_document")?;
                if existing.data != data {
                    return Err(fatal(
                        "state_migration_identity_conflict",
                        "migrate_state_document",
                    ));
                }
            }
            Some(_) => {
                return Err(fatal(
                    "state_migration_schema_conflict",
                    "migrate_state_document",
                ));
            }
        }
        let integrity_tag = self.integrity_tag(
            "state-migration-v1",
            &[migration_id.as_bytes(), data_json.as_slice()],
        );
        if existing_migration.is_none() {
            transaction
                .execute(
                    "INSERT INTO state_migrations
                     (migration_id, data_json, integrity_tag) VALUES (?1, ?2, ?3)",
                    params![migration_id, data_json, integrity_tag],
                )
                .map_err(|_| fatal("state_migration_write_failed", "migrate_state_document"))?;
        }
        transaction
            .commit()
            .map_err(|_| fatal("state_transaction_commit_failed", "migrate_state_document"))?;
        Ok(data)
    }

    pub fn rollback_json_document(
        &self,
        state_key: &str,
        target_revision: u64,
        expected_payload_sha256: &str,
    ) -> RuntimeStateResult<StateDocument> {
        validate_state_key(state_key)?;
        if target_revision == 0 {
            return Err(request(
                "state_rollback_revision_invalid",
                "rollback_state_document",
            ));
        }
        let mut connection = self.connection("rollback_state_document")?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| fatal("state_transaction_begin_failed", "rollback_state_document"))?;
        let current = query_document(&transaction, state_key)?
            .ok_or_else(|| request("state_document_missing", "rollback_state_document"))?;
        let current = self.validate_document_row(current, "rollback_state_document")?;
        if current.payload_sha256() != expected_payload_sha256 {
            return Err(request("state_document_changed", "rollback_state_document"));
        }
        if target_revision >= current.revision() {
            return Err(request(
                "state_rollback_revision_not_older",
                "rollback_state_document",
            ));
        }
        let target = query_document_revision(&transaction, state_key, target_revision)?
            .ok_or_else(|| request("state_rollback_revision_missing", "rollback_state_document"))?;
        let target = self.validate_document_row(target, "rollback_state_document")?;
        let revision = current
            .revision()
            .checked_add(1)
            .ok_or_else(|| fatal("state_revision_overflow", "rollback_state_document"))?;
        let integrity_tag = self.document_integrity_tag(
            state_key,
            target.schema_version(),
            revision,
            target.payload(),
            target.payload_sha256(),
            Some(current.payload_sha256()),
        );
        insert_document_revision(
            &transaction,
            state_key,
            target.schema_version(),
            revision,
            target.payload(),
            target.payload_sha256(),
            Some(current.payload_sha256()),
            &integrity_tag,
        )?;
        transaction
            .commit()
            .map_err(|_| fatal("state_transaction_commit_failed", "rollback_state_document"))?;
        Ok(StateDocument {
            state_key: state_key.to_owned(),
            schema_version: target.schema_version().to_owned(),
            revision,
            payload: target.payload().to_vec(),
            payload_sha256: target.payload_sha256().to_owned(),
            previous_payload_sha256: Some(current.payload_sha256().to_owned()),
        })
    }

    pub fn migrations(&self) -> RuntimeStateResult<Vec<StateMigrationData>> {
        let connection = self.connection("read_state_migrations")?;
        let mut statement = connection
            .prepare(
                "SELECT migration_id, data_json, integrity_tag
                 FROM state_migrations ORDER BY migration_id",
            )
            .map_err(|_| fatal("state_migration_query_failed", "read_state_migrations"))?;
        let rows = statement
            .query_map([], |row| {
                Ok(MigrationRow {
                    migration_id: row.get(0)?,
                    data_json: row.get(1)?,
                    integrity_tag: row.get(2)?,
                })
            })
            .map_err(|_| fatal("state_migration_query_failed", "read_state_migrations"))?;
        rows.map(|row| {
            let row =
                row.map_err(|_| fatal("state_migration_read_failed", "read_state_migrations"))?;
            self.validate_migration_row(&row, "read_state_migrations")
                .map(|row| row.data)
        })
        .collect()
    }

    pub fn stage_release(
        &self,
        manifest: RuntimeReleaseSet,
        sources: &ReleaseArtifactSources,
    ) -> RuntimeStateResult<StagedRelease> {
        manifest
            .validate()
            .map_err(|_| request("release_manifest_invalid", "stage_release"))?;
        self.publish_release_artifacts(&manifest, sources)?;
        let manifest_json = serde_json::to_vec(&manifest)
            .map_err(|_| fatal("release_manifest_encode_failed", "stage_release"))?;
        let manifest_sha256 = manifest.manifest_sha256();
        if manifest_sha256 != sha256_release_manifest(&manifest_json, manifest.release_id()) {
            return Err(fatal("release_manifest_hash_mismatch", "stage_release"));
        }
        let integrity_tag = self.integrity_tag(
            "release-generation-v1",
            &[
                manifest.release_id().as_bytes(),
                manifest_sha256.as_bytes(),
                manifest_json.as_slice(),
            ],
        );
        let connection = self.connection("stage_release")?;
        let created = connection
            .execute(
                "INSERT OR IGNORE INTO release_generations
                 (release_id, manifest_json, manifest_sha256, integrity_tag)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    manifest.release_id(),
                    manifest_json,
                    manifest_sha256,
                    integrity_tag
                ],
            )
            .map_err(|_| fatal("release_generation_write_failed", "stage_release"))?
            == 1;
        if !created {
            let existing = query_release(&connection, manifest.release_id())?
                .ok_or_else(|| fatal("release_generation_missing", "stage_release"))?;
            let existing = self.validate_release_row(existing, "stage_release")?;
            if existing != manifest {
                return Err(fatal(
                    "release_generation_identity_conflict",
                    "stage_release",
                ));
            }
        }
        Ok(StagedRelease { manifest, created })
    }

    pub fn resolve_active_release_artifact(
        &self,
        key: &ReleaseArtifactKey,
    ) -> RuntimeStateResult<ResolvedReleaseArtifact> {
        let active = self
            .active_release()?
            .ok_or_else(|| request("release_active_missing", "resolve_release_artifact"))?;
        let content_sha256 = match key {
            ReleaseArtifactKey::Runtime => active.manifest.runtime_content_sha256(),
            ReleaseArtifactKey::Ui => active.manifest.ui_content_sha256(),
            ReleaseArtifactKey::Resource(resource_id) => active
                .manifest
                .resources()
                .iter()
                .find(|resource| resource.resource_id() == resource_id)
                .map(|resource| resource.content_sha256())
                .ok_or_else(|| request("release_resource_unknown", "resolve_release_artifact"))?,
        };
        let path = self.release_blob_path(content_sha256, "resolve_release_artifact")?;
        verify_release_blob(&path, content_sha256, "resolve_release_artifact")?;
        Ok(ResolvedReleaseArtifact {
            release_id: active.manifest.release_id().to_owned(),
            content_sha256: content_sha256.to_owned(),
            path,
        })
    }

    pub fn release_generations(&self) -> RuntimeStateResult<Vec<RuntimeReleaseSet>> {
        let connection = self.connection("read_release_generations")?;
        let mut statement = connection
            .prepare(
                "SELECT release_id, manifest_json, manifest_sha256, integrity_tag
                 FROM release_generations ORDER BY release_id",
            )
            .map_err(|_| {
                fatal(
                    "release_generation_query_failed",
                    "read_release_generations",
                )
            })?;
        let rows = statement.query_map([], map_release_row).map_err(|_| {
            fatal(
                "release_generation_query_failed",
                "read_release_generations",
            )
        })?;
        rows.map(|row| {
            self.validate_release_row(
                row.map_err(|_| {
                    fatal("release_generation_read_failed", "read_release_generations")
                })?,
                "read_release_generations",
            )
        })
        .collect()
    }

    pub fn active_release(&self) -> RuntimeStateResult<Option<ActiveRelease>> {
        let connection = self.connection("read_active_release")?;
        self.read_active_release(&connection, "read_active_release")
    }

    pub fn preview_release_transition(
        &self,
        kind: ReleaseTransitionKind,
        release_id: &str,
    ) -> RuntimeStateResult<ReleaseTransitionPreview> {
        let connection = self.connection("preview_release_transition")?;
        let target = query_release(&connection, release_id)?
            .ok_or_else(|| request("release_generation_unknown", "preview_release_transition"))?;
        let target = self.validate_release_row(target, "preview_release_transition")?;
        let current = self.read_active_release(&connection, "preview_release_transition")?;
        let previous_release_id = current
            .as_ref()
            .map(|active| active.manifest.release_id().to_owned());
        if previous_release_id.as_deref() == Some(release_id) {
            return Err(request(
                "release_already_active",
                "preview_release_transition",
            ));
        }
        if kind == ReleaseTransitionKind::Rollback {
            let Some(current) = &current else {
                return Err(request(
                    "release_rollback_without_active",
                    "preview_release_transition",
                ));
            };
            if !was_release_active(&connection, release_id)?
                && current.previous_release_id() != Some(release_id)
            {
                return Err(request(
                    "release_rollback_target_not_active_history",
                    "preview_release_transition",
                ));
            }
        }
        let pointer_revision = current
            .as_ref()
            .map_or(Ok(1_u64), |active| active.revision.checked_add(1).ok_or(()))
            .map_err(|()| {
                fatal(
                    "release_pointer_revision_overflow",
                    "preview_release_transition",
                )
            })?;
        let transition_id = release_transition_id(
            kind,
            previous_release_id.as_deref(),
            release_id,
            pointer_revision,
            &target.manifest_sha256(),
        );
        let data = ReleaseTransitionData::new(
            transition_id,
            kind,
            previous_release_id,
            release_id,
            pointer_revision,
            target.manifest_sha256(),
            StateTransitionStatus::new(StateValidationResult::Passed, StateRecoveryAction::None),
        )
        .map_err(|_| request("release_transition_invalid", "preview_release_transition"))?;
        Ok(ReleaseTransitionPreview { data })
    }

    pub fn commit_release_transition(
        &self,
        preview: &ReleaseTransitionPreview,
    ) -> RuntimeStateResult<ActiveRelease> {
        preview
            .data
            .validate()
            .map_err(|_| request("release_transition_invalid", "commit_release_transition"))?;
        let mut connection = self.connection("commit_release_transition")?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| {
                fatal(
                    "state_transaction_begin_failed",
                    "commit_release_transition",
                )
            })?;
        if let Some(existing) =
            query_release_transition(&transaction, preview.data.transition_id())?
        {
            let existing =
                self.validate_release_transition_row(&existing, "commit_release_transition")?;
            if existing != preview.data {
                return Err(fatal(
                    "release_transition_identity_conflict",
                    "commit_release_transition",
                ));
            }
            let active = self
                .read_active_release(&transaction, "commit_release_transition")?
                .ok_or_else(|| {
                    fatal(
                        "release_pointer_missing_after_transition",
                        "commit_release_transition",
                    )
                })?;
            transaction.commit().map_err(|_| {
                fatal(
                    "state_transaction_commit_failed",
                    "commit_release_transition",
                )
            })?;
            return Ok(active);
        }
        let current = self.read_active_release(&transaction, "commit_release_transition")?;
        let expected_revision = current
            .as_ref()
            .map_or(Some(1), |active| active.revision.checked_add(1));
        if current.as_ref().map(|active| active.manifest.release_id())
            != preview.data.previous_release_id()
            || expected_revision != Some(preview.data.pointer_revision())
        {
            return Err(request(
                "release_pointer_changed",
                "commit_release_transition",
            ));
        }
        let target = query_release(&transaction, preview.data.release_id())?
            .ok_or_else(|| request("release_generation_unknown", "commit_release_transition"))?;
        let target = self.validate_release_row(target, "commit_release_transition")?;
        if target.manifest_sha256() != preview.data.manifest_sha256() {
            return Err(fatal(
                "release_transition_manifest_changed",
                "commit_release_transition",
            ));
        }
        if preview.data.kind() == ReleaseTransitionKind::Rollback
            && !was_release_active(&transaction, preview.data.release_id())?
            && current
                .as_ref()
                .and_then(ActiveRelease::previous_release_id)
                != Some(preview.data.release_id())
        {
            return Err(request(
                "release_rollback_target_not_active_history",
                "commit_release_transition",
            ));
        }
        let pointer_revision = sqlite_integer(
            preview.data.pointer_revision(),
            "release_pointer_revision_overflow",
            "commit_release_transition",
        )?;
        let pointer_tag = self.pointer_integrity_tag(
            preview.data.pointer_revision(),
            preview.data.release_id(),
            preview.data.previous_release_id(),
        );
        transaction
            .execute(
                "INSERT INTO release_pointer_history
                 (revision, release_id, previous_release_id, integrity_tag)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    pointer_revision,
                    preview.data.release_id(),
                    preview.data.previous_release_id(),
                    pointer_tag
                ],
            )
            .map_err(|_| {
                fatal(
                    "release_pointer_history_write_failed",
                    "commit_release_transition",
                )
            })?;
        transaction
            .execute(
                "INSERT INTO release_pointer
                 (singleton, revision, release_id, previous_release_id, integrity_tag)
                 VALUES (1, ?1, ?2, ?3, ?4)
                 ON CONFLICT(singleton) DO UPDATE SET
                    revision = excluded.revision,
                    release_id = excluded.release_id,
                    previous_release_id = excluded.previous_release_id,
                    integrity_tag = excluded.integrity_tag",
                params![
                    pointer_revision,
                    preview.data.release_id(),
                    preview.data.previous_release_id(),
                    pointer_tag
                ],
            )
            .map_err(|_| fatal("release_pointer_write_failed", "commit_release_transition"))?;
        let data_json = serde_json::to_vec(&preview.data).map_err(|_| {
            fatal(
                "release_transition_encode_failed",
                "commit_release_transition",
            )
        })?;
        let transition_tag = self.integrity_tag(
            "release-transition-v1",
            &[
                preview.data.transition_id().as_bytes(),
                data_json.as_slice(),
            ],
        );
        transaction
            .execute(
                "INSERT INTO release_transitions
                 (transition_id, data_json, integrity_tag) VALUES (?1, ?2, ?3)",
                params![preview.data.transition_id(), data_json, transition_tag],
            )
            .map_err(|_| {
                fatal(
                    "release_transition_write_failed",
                    "commit_release_transition",
                )
            })?;
        transaction.commit().map_err(|_| {
            fatal(
                "state_transaction_commit_failed",
                "commit_release_transition",
            )
        })?;
        Ok(ActiveRelease {
            revision: preview.data.pointer_revision(),
            manifest: target,
            previous_release_id: preview.data.previous_release_id().map(str::to_owned),
        })
    }

    pub fn release_transitions(&self) -> RuntimeStateResult<Vec<ReleaseTransitionData>> {
        let connection = self.connection("read_release_transitions")?;
        let mut statement = connection
            .prepare(
                "SELECT transition_id, data_json, integrity_tag
                 FROM release_transitions ORDER BY transition_id",
            )
            .map_err(|_| {
                fatal(
                    "release_transition_query_failed",
                    "read_release_transitions",
                )
            })?;
        let rows = statement
            .query_map([], |row| {
                Ok(TransitionRow {
                    transition_id: row.get(0)?,
                    data_json: row.get(1)?,
                    integrity_tag: row.get(2)?,
                })
            })
            .map_err(|_| {
                fatal(
                    "release_transition_query_failed",
                    "read_release_transitions",
                )
            })?;
        rows.map(|row| {
            let row = row
                .map_err(|_| fatal("release_transition_read_failed", "read_release_transitions"))?;
            self.validate_release_transition_row(&row, "read_release_transitions")
        })
        .collect()
    }

    fn validate_all(&self) -> RuntimeStateResult<()> {
        let connection = self.connection("validate_runtime_state")?;
        validate_document_tables(self, &connection)?;
        validate_pointer_tables(self, &connection)?;
        drop(connection);
        self.migrations()?;
        self.release_generations()?;
        self.release_transitions()?;
        Ok(())
    }

    fn read_active_release(
        &self,
        connection: &Connection,
        operation: &'static str,
    ) -> RuntimeStateResult<Option<ActiveRelease>> {
        let pointer = query_pointer(connection)?;
        let Some(pointer) = pointer else {
            return Ok(None);
        };
        self.validate_pointer_row(&pointer, operation)?;
        let manifest = query_release(connection, &pointer.release_id)?
            .ok_or_else(|| fatal("release_pointer_target_missing", operation))?;
        let manifest = self.validate_release_row(manifest, operation)?;
        Ok(Some(ActiveRelease {
            revision: pointer.revision,
            manifest,
            previous_release_id: pointer.previous_release_id,
        }))
    }

    fn validate_document_row(
        &self,
        row: DocumentRow,
        operation: &'static str,
    ) -> RuntimeStateResult<StateDocument> {
        validate_document_input(&row.state_key, &row.schema_version, &row.payload)
            .map_err(|_| fatal("state_document_invalid", operation))?;
        if row.revision == 0
            || row.payload_sha256 != sha256(&row.payload)
            || row.integrity_tag
                != self.document_integrity_tag(
                    &row.state_key,
                    &row.schema_version,
                    row.revision,
                    &row.payload,
                    &row.payload_sha256,
                    row.previous_payload_sha256.as_deref(),
                )
        {
            return Err(fatal("state_document_integrity_mismatch", operation));
        }
        Ok(StateDocument {
            state_key: row.state_key,
            schema_version: row.schema_version,
            revision: row.revision,
            payload: row.payload,
            payload_sha256: row.payload_sha256,
            previous_payload_sha256: row.previous_payload_sha256,
        })
    }

    fn validate_migration_row(
        &self,
        row: &MigrationRow,
        operation: &'static str,
    ) -> RuntimeStateResult<ValidatedMigration> {
        let data = serde_json::from_slice::<StateMigrationData>(&row.data_json)
            .map_err(|_| fatal("state_migration_invalid", operation))?;
        data.validate()
            .map_err(|_| fatal("state_migration_invalid", operation))?;
        if data.migration_id() != row.migration_id
            || row.integrity_tag
                != self.integrity_tag(
                    "state-migration-v1",
                    &[row.migration_id.as_bytes(), row.data_json.as_slice()],
                )
        {
            return Err(fatal("state_migration_integrity_mismatch", operation));
        }
        Ok(ValidatedMigration { data })
    }

    fn validate_release_row(
        &self,
        row: ReleaseRow,
        operation: &'static str,
    ) -> RuntimeStateResult<RuntimeReleaseSet> {
        let manifest = serde_json::from_slice::<RuntimeReleaseSet>(&row.manifest_json)
            .map_err(|_| fatal("release_manifest_invalid", operation))?;
        manifest
            .validate()
            .map_err(|_| fatal("release_manifest_invalid", operation))?;
        if manifest.release_id() != row.release_id
            || manifest.manifest_sha256() != row.manifest_sha256
            || row.manifest_sha256 != sha256_release_manifest(&row.manifest_json, &row.release_id)
            || row.integrity_tag
                != self.integrity_tag(
                    "release-generation-v1",
                    &[
                        row.release_id.as_bytes(),
                        row.manifest_sha256.as_bytes(),
                        row.manifest_json.as_slice(),
                    ],
                )
        {
            return Err(fatal("release_generation_integrity_mismatch", operation));
        }
        self.verify_release_artifacts(&manifest, operation)?;
        Ok(manifest)
    }

    fn publish_release_artifacts(
        &self,
        manifest: &RuntimeReleaseSet,
        sources: &ReleaseArtifactSources,
    ) -> RuntimeStateResult<()> {
        let expected_resources = manifest
            .resources()
            .iter()
            .map(|resource| resource.resource_id().to_owned())
            .collect::<BTreeSet<_>>();
        if sources.resources.keys().cloned().collect::<BTreeSet<_>>() != expected_resources {
            return Err(request(
                "release_artifact_sources_mismatch",
                "stage_release",
            ));
        }
        self.publish_release_artifact(sources.runtime(), manifest.runtime_content_sha256())?;
        self.publish_release_artifact(sources.ui(), manifest.ui_content_sha256())?;
        for resource in manifest.resources() {
            let source = sources
                .resources
                .get(resource.resource_id())
                .ok_or_else(|| request("release_artifact_sources_mismatch", "stage_release"))?;
            self.publish_release_artifact(source, resource.content_sha256())?;
        }
        Ok(())
    }

    fn publish_release_artifact(
        &self,
        source: &Path,
        expected_sha256: &str,
    ) -> RuntimeStateResult<()> {
        let destination = self.release_blob_path(expected_sha256, "stage_release")?;
        if destination.exists() {
            return verify_release_blob(&destination, expected_sha256, "stage_release");
        }
        require_release_regular_file(source, "release_artifact_source_invalid", "stage_release")?;
        let digest = expected_sha256
            .strip_prefix("sha256:")
            .ok_or_else(|| request("release_artifact_hash_invalid", "stage_release"))?;
        let temporary = self.release_blobs.join(format!(
            ".{digest}.{}.{}.tmp",
            std::process::id(),
            RELEASE_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let result = copy_and_hash_release_artifact(source, &temporary, expected_sha256);
        if let Err(error) = result {
            remove_release_temporary(&temporary)?;
            return Err(error);
        }
        match fs::rename(&temporary, &destination) {
            Ok(()) => sync_state_directory(&self.release_blobs)?,
            Err(_) if destination.exists() => {
                remove_release_temporary(&temporary)?;
                verify_release_blob(&destination, expected_sha256, "stage_release")?;
            }
            Err(_) => {
                remove_release_temporary(&temporary)?;
                return Err(fatal("release_artifact_publish_failed", "stage_release"));
            }
        }
        verify_release_blob(&destination, expected_sha256, "stage_release")
    }

    fn verify_release_artifacts(
        &self,
        manifest: &RuntimeReleaseSet,
        operation: &'static str,
    ) -> RuntimeStateResult<()> {
        for content_sha256 in std::iter::once(manifest.runtime_content_sha256())
            .chain(std::iter::once(manifest.ui_content_sha256()))
            .chain(
                manifest
                    .resources()
                    .iter()
                    .map(|resource| resource.content_sha256()),
            )
        {
            let path = self.release_blob_path(content_sha256, operation)?;
            verify_release_blob(&path, content_sha256, operation)?;
        }
        Ok(())
    }

    fn release_blob_path(
        &self,
        content_sha256: &str,
        operation: &'static str,
    ) -> RuntimeStateResult<PathBuf> {
        let digest = content_sha256
            .strip_prefix("sha256:")
            .filter(|digest| {
                digest.len() == 64
                    && digest
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
            })
            .ok_or_else(|| request("release_artifact_hash_invalid", operation))?;
        Ok(self.release_blobs.join(digest))
    }

    fn validate_pointer_row(
        &self,
        row: &PointerRow,
        operation: &'static str,
    ) -> RuntimeStateResult<()> {
        if row.revision == 0
            || row.previous_release_id.as_deref() == Some(row.release_id.as_str())
            || row.integrity_tag
                != self.pointer_integrity_tag(
                    row.revision,
                    &row.release_id,
                    row.previous_release_id.as_deref(),
                )
        {
            return Err(fatal("release_pointer_integrity_mismatch", operation));
        }
        Ok(())
    }

    fn validate_release_transition_row(
        &self,
        row: &TransitionRow,
        operation: &'static str,
    ) -> RuntimeStateResult<ReleaseTransitionData> {
        let data = serde_json::from_slice::<ReleaseTransitionData>(&row.data_json)
            .map_err(|_| fatal("release_transition_invalid", operation))?;
        data.validate()
            .map_err(|_| fatal("release_transition_invalid", operation))?;
        if data.transition_id() != row.transition_id
            || row.integrity_tag
                != self.integrity_tag(
                    "release-transition-v1",
                    &[row.transition_id.as_bytes(), row.data_json.as_slice()],
                )
        {
            return Err(fatal("release_transition_integrity_mismatch", operation));
        }
        Ok(data)
    }

    fn document_integrity_tag(
        &self,
        state_key: &str,
        schema_version: &str,
        revision: u64,
        payload: &[u8],
        payload_sha256: &str,
        previous_payload_sha256: Option<&str>,
    ) -> String {
        let revision = revision.to_be_bytes();
        self.integrity_tag(
            "state-document-v1",
            &[
                state_key.as_bytes(),
                schema_version.as_bytes(),
                &revision,
                payload,
                payload_sha256.as_bytes(),
                previous_payload_sha256.unwrap_or_default().as_bytes(),
            ],
        )
    }

    fn pointer_integrity_tag(
        &self,
        revision: u64,
        release_id: &str,
        previous_release_id: Option<&str>,
    ) -> String {
        let revision = revision.to_be_bytes();
        self.integrity_tag(
            "release-pointer-v1",
            &[
                &revision,
                release_id.as_bytes(),
                previous_release_id.unwrap_or_default().as_bytes(),
            ],
        )
    }

    fn integrity_tag(&self, domain: &str, fields: &[&[u8]]) -> String {
        let mut digest = Sha256::new();
        digest.update(b"actingcommand-keyed-integrity-v1\0");
        update_field(&mut digest, domain.as_bytes());
        update_field(&mut digest, &self.integrity_key);
        for field in fields {
            update_field(&mut digest, field);
        }
        format!("sha256:{:x}", digest.finalize())
    }

    fn connection(
        &self,
        operation: &'static str,
    ) -> RuntimeStateResult<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| fatal("state_connection_poisoned", operation))
    }
}

struct DocumentRow {
    state_key: String,
    schema_version: String,
    revision: u64,
    payload: Vec<u8>,
    payload_sha256: String,
    previous_payload_sha256: Option<String>,
    integrity_tag: String,
}

struct MigrationRow {
    migration_id: String,
    data_json: Vec<u8>,
    integrity_tag: String,
}

struct ValidatedMigration {
    data: StateMigrationData,
}

struct ReleaseRow {
    release_id: String,
    manifest_json: Vec<u8>,
    manifest_sha256: String,
    integrity_tag: String,
}

struct PointerRow {
    revision: u64,
    release_id: String,
    previous_release_id: Option<String>,
    integrity_tag: String,
}

struct TransitionRow {
    transition_id: String,
    data_json: Vec<u8>,
    integrity_tag: String,
}

fn query_document(
    connection: &Connection,
    state_key: &str,
) -> RuntimeStateResult<Option<DocumentRow>> {
    connection
        .query_row(
            "SELECT state_key, schema_version, revision, payload, payload_sha256,
                    previous_payload_sha256, integrity_tag
             FROM state_documents WHERE state_key = ?1",
            [state_key],
            map_document_row,
        )
        .optional()
        .map_err(|_| fatal("state_document_query_failed", "query_state_document"))
}

fn query_document_revision(
    connection: &Connection,
    state_key: &str,
    revision: u64,
) -> RuntimeStateResult<Option<DocumentRow>> {
    let revision = sqlite_integer(
        revision,
        "state_revision_overflow",
        "query_state_document_revision",
    )?;
    connection
        .query_row(
            "SELECT state_key, schema_version, revision, payload, payload_sha256,
                    previous_payload_sha256, integrity_tag
             FROM state_document_history WHERE state_key = ?1 AND revision = ?2",
            params![state_key, revision],
            map_document_row,
        )
        .optional()
        .map_err(|_| {
            fatal(
                "state_history_query_failed",
                "query_state_document_revision",
            )
        })
}

fn map_document_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DocumentRow> {
    Ok(DocumentRow {
        state_key: row.get(0)?,
        schema_version: row.get(1)?,
        revision: row_u64(row, 2)?,
        payload: row.get(3)?,
        payload_sha256: row.get(4)?,
        previous_payload_sha256: row.get(5)?,
        integrity_tag: row.get(6)?,
    })
}

#[allow(clippy::too_many_arguments)]
fn insert_document_revision(
    transaction: &Transaction<'_>,
    state_key: &str,
    schema_version: &str,
    revision: u64,
    payload: &[u8],
    payload_sha256: &str,
    previous_payload_sha256: Option<&str>,
    integrity_tag: &str,
) -> RuntimeStateResult<()> {
    let revision = sqlite_integer(revision, "state_revision_overflow", "write_state_document")?;
    transaction
        .execute(
            "INSERT INTO state_document_history
             (state_key, schema_version, revision, payload, payload_sha256,
              previous_payload_sha256, integrity_tag)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                state_key,
                schema_version,
                revision,
                payload,
                payload_sha256,
                previous_payload_sha256,
                integrity_tag
            ],
        )
        .map_err(|_| fatal("state_history_write_failed", "write_state_document"))?;
    transaction
        .execute(
            "INSERT INTO state_documents
             (state_key, schema_version, revision, payload, payload_sha256,
              previous_payload_sha256, integrity_tag)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(state_key) DO UPDATE SET
                schema_version = excluded.schema_version,
                revision = excluded.revision,
                payload = excluded.payload,
                payload_sha256 = excluded.payload_sha256,
                previous_payload_sha256 = excluded.previous_payload_sha256,
                integrity_tag = excluded.integrity_tag",
            params![
                state_key,
                schema_version,
                revision,
                payload,
                payload_sha256,
                previous_payload_sha256,
                integrity_tag
            ],
        )
        .map_err(|_| fatal("state_document_write_failed", "write_state_document"))?;
    Ok(())
}

fn query_migration(
    connection: &Connection,
    migration_id: &str,
) -> RuntimeStateResult<Option<MigrationRow>> {
    connection
        .query_row(
            "SELECT migration_id, data_json, integrity_tag
             FROM state_migrations WHERE migration_id = ?1",
            [migration_id],
            |row| {
                Ok(MigrationRow {
                    migration_id: row.get(0)?,
                    data_json: row.get(1)?,
                    integrity_tag: row.get(2)?,
                })
            },
        )
        .optional()
        .map_err(|_| fatal("state_migration_query_failed", "query_state_migration"))
}

fn query_release(
    connection: &Connection,
    release_id: &str,
) -> RuntimeStateResult<Option<ReleaseRow>> {
    connection
        .query_row(
            "SELECT release_id, manifest_json, manifest_sha256, integrity_tag
             FROM release_generations WHERE release_id = ?1",
            [release_id],
            map_release_row,
        )
        .optional()
        .map_err(|_| {
            fatal(
                "release_generation_query_failed",
                "query_release_generation",
            )
        })
}

fn map_release_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ReleaseRow> {
    Ok(ReleaseRow {
        release_id: row.get(0)?,
        manifest_json: row.get(1)?,
        manifest_sha256: row.get(2)?,
        integrity_tag: row.get(3)?,
    })
}

fn query_pointer(connection: &Connection) -> RuntimeStateResult<Option<PointerRow>> {
    connection
        .query_row(
            "SELECT revision, release_id, previous_release_id, integrity_tag
             FROM release_pointer WHERE singleton = 1",
            [],
            |row| {
                Ok(PointerRow {
                    revision: row_u64(row, 0)?,
                    release_id: row.get(1)?,
                    previous_release_id: row.get(2)?,
                    integrity_tag: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(|_| fatal("release_pointer_query_failed", "query_release_pointer"))
}

fn query_release_transition(
    connection: &Connection,
    transition_id: &str,
) -> RuntimeStateResult<Option<TransitionRow>> {
    connection
        .query_row(
            "SELECT transition_id, data_json, integrity_tag
             FROM release_transitions WHERE transition_id = ?1",
            [transition_id],
            |row| {
                Ok(TransitionRow {
                    transition_id: row.get(0)?,
                    data_json: row.get(1)?,
                    integrity_tag: row.get(2)?,
                })
            },
        )
        .optional()
        .map_err(|_| {
            fatal(
                "release_transition_query_failed",
                "query_release_transition",
            )
        })
}

fn was_release_active(connection: &Connection, release_id: &str) -> RuntimeStateResult<bool> {
    connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM release_pointer_history WHERE release_id = ?1
             )",
            [release_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|_| fatal("release_history_query_failed", "query_release_history"))
}

fn validate_document_tables(
    store: &RuntimeStateStore,
    connection: &Connection,
) -> RuntimeStateResult<()> {
    let mut statement = connection
        .prepare(
            "SELECT state_key, schema_version, revision, payload, payload_sha256,
                    previous_payload_sha256, integrity_tag
             FROM state_document_history ORDER BY state_key, revision",
        )
        .map_err(|_| fatal("state_history_query_failed", "validate_runtime_state"))?;
    let rows = statement
        .query_map([], map_document_row)
        .map_err(|_| fatal("state_history_query_failed", "validate_runtime_state"))?;
    let mut history = BTreeSet::new();
    for row in rows {
        let row = row.map_err(|_| fatal("state_history_read_failed", "validate_runtime_state"))?;
        history.insert((
            row.state_key.clone(),
            row.revision,
            row.payload_sha256.clone(),
        ));
        store.validate_document_row(row, "validate_runtime_state")?;
    }
    let mut statement = connection
        .prepare(
            "SELECT state_key, schema_version, revision, payload, payload_sha256,
                    previous_payload_sha256, integrity_tag
             FROM state_documents ORDER BY state_key",
        )
        .map_err(|_| fatal("state_document_query_failed", "validate_runtime_state"))?;
    let rows = statement
        .query_map([], map_document_row)
        .map_err(|_| fatal("state_document_query_failed", "validate_runtime_state"))?;
    for row in rows {
        let row = row.map_err(|_| fatal("state_document_read_failed", "validate_runtime_state"))?;
        if !history.contains(&(
            row.state_key.clone(),
            row.revision,
            row.payload_sha256.clone(),
        )) {
            return Err(fatal(
                "state_document_history_missing",
                "validate_runtime_state",
            ));
        }
        store.validate_document_row(row, "validate_runtime_state")?;
    }
    Ok(())
}

fn validate_pointer_tables(
    store: &RuntimeStateStore,
    connection: &Connection,
) -> RuntimeStateResult<()> {
    let mut statement = connection
        .prepare(
            "SELECT revision, release_id, previous_release_id, integrity_tag
             FROM release_pointer_history ORDER BY revision",
        )
        .map_err(|_| fatal("release_history_query_failed", "validate_runtime_state"))?;
    let rows = statement
        .query_map([], |row| {
            Ok(PointerRow {
                revision: row_u64(row, 0)?,
                release_id: row.get(1)?,
                previous_release_id: row.get(2)?,
                integrity_tag: row.get(3)?,
            })
        })
        .map_err(|_| fatal("release_history_query_failed", "validate_runtime_state"))?;
    let mut revisions = BTreeSet::new();
    for row in rows {
        let row =
            row.map_err(|_| fatal("release_history_read_failed", "validate_runtime_state"))?;
        store.validate_pointer_row(&row, "validate_runtime_state")?;
        revisions.insert(row.revision);
    }
    if let Some(pointer) = query_pointer(connection)? {
        store.validate_pointer_row(&pointer, "validate_runtime_state")?;
        if !revisions.contains(&pointer.revision) {
            return Err(fatal(
                "release_pointer_history_missing",
                "validate_runtime_state",
            ));
        }
    }
    Ok(())
}

fn validate_document_input(
    state_key: &str,
    schema_version: &str,
    payload: &[u8],
) -> RuntimeStateResult<()> {
    validate_state_key(state_key)?;
    validate_version(schema_version, "validate_state_document")?;
    if payload.is_empty() || payload.len() > MAX_STATE_DOCUMENT_BYTES {
        return Err(request(
            "state_document_size_invalid",
            "validate_state_document",
        ));
    }
    serde_json::from_slice::<serde_json::Value>(payload)
        .map_err(|_| request("state_document_json_invalid", "validate_state_document"))?;
    Ok(())
}

fn validate_state_key(value: &str) -> RuntimeStateResult<()> {
    if value.is_empty()
        || value.len() > 128
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'.' | b'_' | b':' | b'-')
        })
    {
        return Err(request("state_key_invalid", "validate_state_key"));
    }
    Ok(())
}

fn validate_version(value: &str, operation: &'static str) -> RuntimeStateResult<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'+'))
    {
        return Err(request("state_version_invalid", operation));
    }
    Ok(())
}

fn prepare_release_blob_store(root: &Path) -> RuntimeStateResult<PathBuf> {
    let path = root.join(RUNTIME_RELEASE_BLOB_DIRECTORY);
    if path.exists() {
        require_regular_directory(&path)?;
    } else {
        fs::create_dir(&path)
            .map_err(|_| fatal("release_artifact_root_create_failed", "open_runtime_state"))?;
        require_regular_directory(&path)?;
        sync_state_directory(root)?;
    }
    Ok(path)
}

fn copy_and_hash_release_artifact(
    source: &Path,
    destination: &Path,
    expected_sha256: &str,
) -> RuntimeStateResult<()> {
    let mut source =
        File::open(source).map_err(|_| fatal("release_artifact_read_failed", "stage_release"))?;
    let mut destination = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)
        .map_err(|_| fatal("release_artifact_stage_failed", "stage_release"))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = source
            .read(&mut buffer)
            .map_err(|_| fatal("release_artifact_read_failed", "stage_release"))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        destination
            .write_all(&buffer[..read])
            .map_err(|_| fatal("release_artifact_stage_failed", "stage_release"))?;
    }
    destination
        .sync_all()
        .map_err(|_| fatal("release_artifact_stage_failed", "stage_release"))?;
    let actual = format!("sha256:{:x}", digest.finalize());
    if actual != expected_sha256 {
        return Err(fatal("release_artifact_hash_mismatch", "stage_release"));
    }
    Ok(())
}

fn verify_release_blob(
    path: &Path,
    expected_sha256: &str,
    operation: &'static str,
) -> RuntimeStateResult<()> {
    require_release_regular_file(path, "release_artifact_unavailable", operation)?;
    let mut file =
        File::open(path).map_err(|_| fatal("release_artifact_read_failed", operation))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| fatal("release_artifact_read_failed", operation))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    if format!("sha256:{:x}", digest.finalize()) != expected_sha256 {
        return Err(fatal("release_artifact_hash_mismatch", operation));
    }
    Ok(())
}

fn require_release_regular_file(
    path: &Path,
    code: &'static str,
    operation: &'static str,
) -> RuntimeStateResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(|_| fatal(code, operation))?;
    if !metadata.is_file() || is_link_or_reparse(&metadata) {
        return Err(fatal(code, operation));
    }
    Ok(())
}

fn remove_release_temporary(path: &Path) -> RuntimeStateResult<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(fatal("release_artifact_cleanup_failed", "stage_release")),
    }
}

fn require_regular_directory(path: &Path) -> RuntimeStateResult<()> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| fatal("state_root_inspect_failed", "open_runtime_state"))?;
    if !metadata.is_dir() || is_link_or_reparse(&metadata) {
        return Err(fatal("state_root_unsafe", "open_runtime_state"));
    }
    Ok(())
}

fn require_regular_file(path: &Path) -> RuntimeStateResult<()> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| fatal("state_database_inspect_failed", "open_runtime_state"))?;
    if !metadata.is_file() || is_link_or_reparse(&metadata) {
        return Err(fatal("state_database_unsafe", "open_runtime_state"));
    }
    Ok(())
}

fn load_or_create_integrity_key(
    root: &Path,
    bootstrap_seed: &[u8],
    database_existed: bool,
) -> RuntimeStateResult<Box<[u8]>> {
    let path = root.join(RUNTIME_STATE_INTEGRITY_KEY_FILE);
    if path.exists() {
        require_regular_file(&path)?;
        let bytes = fs::read(&path)
            .map_err(|_| fatal("state_integrity_key_read_failed", "open_runtime_state"))?;
        if bytes.len() != 32 {
            return Err(fatal("state_integrity_key_invalid", "open_runtime_state"));
        }
        return Ok(bytes.into_boxed_slice());
    }
    if database_existed {
        return Err(fatal("state_integrity_key_missing", "open_runtime_state"));
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| fatal("state_integrity_key_clock_failed", "open_runtime_state"))?;
    let mut digest = Sha256::new();
    digest.update(b"actingcommand-runtime-state-key-v1\0");
    update_field(&mut digest, bootstrap_seed);
    update_field(&mut digest, &now.as_nanos().to_be_bytes());
    update_field(&mut digest, &std::process::id().to_be_bytes());
    let key = digest.finalize().to_vec();
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .map_err(|_| fatal("state_integrity_key_create_failed", "open_runtime_state"))?;
    file.write_all(&key)
        .and_then(|()| file.sync_all())
        .map_err(|_| fatal("state_integrity_key_write_failed", "open_runtime_state"))?;
    sync_state_directory(root)?;
    Ok(key.into_boxed_slice())
}

#[cfg(unix)]
fn sync_state_directory(path: &Path) -> RuntimeStateResult<()> {
    OpenOptions::new()
        .read(true)
        .open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| fatal("state_directory_sync_failed", "open_runtime_state"))
}

#[cfg(not(unix))]
fn sync_state_directory(_path: &Path) -> RuntimeStateResult<()> {
    // Rust's standard library cannot open Windows directories for fsync without unsafe flags.
    Ok(())
}

#[cfg(windows)]
fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn migration_id(
    state_key: &str,
    from_schema_version: &str,
    to_schema_version: &str,
    payload_sha256: &str,
) -> String {
    format!(
        "migration:{}",
        digest_fields(&[
            b"state-migration-v1",
            state_key.as_bytes(),
            from_schema_version.as_bytes(),
            to_schema_version.as_bytes(),
            payload_sha256.as_bytes(),
        ])
    )
}

fn release_transition_id(
    kind: ReleaseTransitionKind,
    previous_release_id: Option<&str>,
    release_id: &str,
    revision: u64,
    manifest_sha256: &str,
) -> String {
    let revision = revision.to_be_bytes();
    let kind = match kind {
        ReleaseTransitionKind::Activate => b"activate".as_slice(),
        ReleaseTransitionKind::Rollback => b"rollback".as_slice(),
    };
    format!(
        "release-transition:{}",
        digest_fields(&[
            b"release-transition-v1",
            kind,
            previous_release_id.unwrap_or_default().as_bytes(),
            release_id.as_bytes(),
            &revision,
            manifest_sha256.as_bytes(),
        ])
    )
}

fn sha256(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn sha256_release_manifest(_bytes: &[u8], release_id: &str) -> String {
    release_id
        .strip_prefix("release:")
        .map_or_else(String::new, |digest| format!("sha256:{digest}"))
}

fn digest_fields(fields: &[&[u8]]) -> String {
    let mut digest = Sha256::new();
    for field in fields {
        update_field(&mut digest, field);
    }
    format!("{:x}", digest.finalize())
}

fn row_u64(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value = row.get::<_, i64>(index)?;
    u64::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(index, Type::Integer, Box::new(error))
    })
}

fn sqlite_integer(
    value: u64,
    code: &'static str,
    operation: &'static str,
) -> RuntimeStateResult<i64> {
    i64::try_from(value).map_err(|_| fatal(code, operation))
}

fn update_field(digest: &mut Sha256, field: &[u8]) {
    digest.update((field.len() as u64).to_be_bytes());
    digest.update(field);
}

fn request(code: &'static str, operation: &'static str) -> RuntimeStateError {
    RuntimeStateError::request(code, operation)
}

fn fatal(code: &'static str, operation: &'static str) -> RuntimeStateError {
    RuntimeStateError::fatal(code, operation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_contract::ReleaseResourceVersion;
    use std::sync::{Arc, Barrier};
    use std::thread;
    use tempfile::TempDir;

    fn release(
        root: &Path,
        version: &str,
        marker: char,
    ) -> (RuntimeReleaseSet, ReleaseArtifactSources) {
        let source_root = root.join(format!("release-source-{version}-{marker}"));
        fs::create_dir(&source_root).expect("release source root");
        let runtime = source_root.join("runtime.bin");
        let ui = source_root.join("ui.bin");
        let resource = source_root.join("resource.bin");
        let runtime_bytes = format!("runtime:{version}:{marker}");
        let ui_bytes = format!("ui:{version}:{marker}");
        let resource_bytes = format!("resource:{version}:{marker}");
        fs::write(&runtime, runtime_bytes.as_bytes()).expect("runtime artifact");
        fs::write(&ui, ui_bytes.as_bytes()).expect("UI artifact");
        fs::write(&resource, resource_bytes.as_bytes()).expect("resource artifact");
        let manifest = RuntimeReleaseSet::new(
            version,
            sha256(runtime_bytes.as_bytes()),
            version,
            sha256(ui_bytes.as_bytes()),
            vec![
                ReleaseResourceVersion::new(
                    "project-a",
                    version,
                    sha256(resource_bytes.as_bytes()),
                )
                .expect("resource"),
            ],
        )
        .expect("release");
        let sources = ReleaseArtifactSources::new(
            runtime,
            ui,
            BTreeMap::from([("project-a".to_owned(), resource)]),
        );
        (manifest, sources)
    }

    #[test]
    fn document_envelopes_detect_out_of_band_payload_changes() {
        let root = TempDir::new().expect("tempdir");
        let store = RuntimeStateStore::open(root.path(), b"0123456789abcdef").expect("store");
        let first = store
            .write_json_document("policy.active", "pointer.v1", br#"{"value":1}"#, None)
            .expect("first document");
        let second = store
            .write_json_document(
                "policy.active",
                "pointer.v1",
                br#"{"value":2}"#,
                Some(first.payload_sha256()),
            )
            .expect("second document");
        assert_eq!(second.revision(), 2);
        drop(store);

        let connection = Connection::open(root.path().join(RUNTIME_STATE_DATABASE_FILE))
            .expect("tamper connection");
        connection
            .execute(
                "UPDATE state_documents SET payload = ?1 WHERE state_key = 'policy.active'",
                [br#"{"value":9}"#.as_slice()],
            )
            .expect("tamper state");
        drop(connection);
        let error = RuntimeStateStore::open(root.path(), b"0123456789abcdef")
            .err()
            .expect("tamper must fail");
        assert_eq!(error.code(), "state_document_integrity_mismatch");
        assert!(error.is_fatal());
    }

    #[test]
    fn legacy_migration_is_idempotent_and_conflicts_fail_loudly() {
        let root = TempDir::new().expect("tempdir");
        let store = RuntimeStateStore::open(root.path(), b"0123456789abcdef").expect("store");
        let legacy = store
            .write_json_document("policy.active", "pointer-json.v1", br#"{"value":1}"#, None)
            .expect("legacy document");
        let first = store
            .migrate_legacy_json_document(
                "policy.active",
                "pointer-json.v1",
                "pointer.v1",
                br#"{"value":1}"#,
            )
            .expect("migration");
        let replay = store
            .migrate_legacy_json_document(
                "policy.active",
                "pointer-json.v1",
                "pointer.v1",
                br#"{"value":1}"#,
            )
            .expect("migration replay");
        assert_eq!(first, replay);
        assert_eq!(store.migrations().expect("migrations"), vec![first]);
        let migrated = store
            .read_json_document("policy.active")
            .expect("read migrated document")
            .expect("migrated document");
        assert_eq!(migrated.schema_version(), "pointer.v1");
        assert_eq!(migrated.revision(), 2);
        assert_eq!(migrated.payload_sha256(), legacy.payload_sha256());
        assert_eq!(
            store
                .migrate_legacy_json_document(
                    "policy.active",
                    "pointer-json.v1",
                    "pointer.v1",
                    br#"{"value":2}"#,
                )
                .expect_err("migration conflict")
                .code(),
            "state_migration_content_conflict"
        );
        assert_eq!(
            store
                .migrate_legacy_json_document(
                    "policy.active",
                    "pointer.v1",
                    "pointer.v1",
                    br#"{"value":1}"#,
                )
                .expect_err("unchanged schema")
                .code(),
            "state_migration_schema_unchanged"
        );
    }

    #[test]
    fn document_identity_includes_schema_version() {
        let root = TempDir::new().expect("tempdir");
        let store = RuntimeStateStore::open(root.path(), b"0123456789abcdef").expect("store");
        let first = store
            .write_json_document("policy.active", "pointer.v1", br#"{"value":1}"#, None)
            .expect("first document");
        let second = store
            .write_json_document(
                "policy.active",
                "pointer.v2",
                br#"{"value":1}"#,
                Some(first.payload_sha256()),
            )
            .expect("schema-only revision");
        assert_eq!(second.revision(), 2);
        assert_eq!(second.schema_version(), "pointer.v2");
        assert_eq!(second.payload_sha256(), first.payload_sha256());
    }

    #[test]
    fn migration_rejects_unexpected_current_schema() {
        let root = TempDir::new().expect("tempdir");
        let store = RuntimeStateStore::open(root.path(), b"0123456789abcdef").expect("store");
        store
            .write_json_document(
                "policy.active",
                "pointer.unexpected",
                br#"{"value":1}"#,
                None,
            )
            .expect("unexpected document");
        let error = store
            .migrate_legacy_json_document(
                "policy.active",
                "pointer-json.v1",
                "pointer.v1",
                br#"{"value":1}"#,
            )
            .expect_err("unexpected schema must fail");
        assert_eq!(error.code(), "state_migration_schema_conflict");
        assert!(error.is_fatal());
    }

    #[test]
    fn migration_rejects_target_schema_without_matching_record() {
        let root = TempDir::new().expect("tempdir");
        let store = RuntimeStateStore::open(root.path(), b"0123456789abcdef").expect("store");
        store
            .write_json_document("policy.active", "pointer.v1", br#"{"value":1}"#, None)
            .expect("target document");
        let error = store
            .migrate_legacy_json_document(
                "policy.active",
                "pointer-json.v1",
                "pointer.v1",
                br#"{"value":1}"#,
            )
            .expect_err("missing migration record must fail");
        assert_eq!(error.code(), "state_migration_record_missing");
        assert!(error.is_fatal());
    }

    #[test]
    fn release_generations_switch_atomically_and_rollback_only_to_history() {
        let root = TempDir::new().expect("tempdir");
        let store = RuntimeStateStore::open(root.path(), b"0123456789abcdef").expect("store");
        let (first, first_sources) = release(root.path(), "1.0.0", 'a');
        let (second, second_sources) = release(root.path(), "2.0.0", 'b');
        let (never_active, never_active_sources) = release(root.path(), "3.0.0", 'c');
        for (manifest, sources) in [
            (first.clone(), first_sources),
            (second.clone(), second_sources),
            (never_active.clone(), never_active_sources),
        ] {
            assert!(
                store
                    .stage_release(manifest, &sources)
                    .expect("stage")
                    .created()
            );
        }
        let activate_first = store
            .preview_release_transition(ReleaseTransitionKind::Activate, first.release_id())
            .expect("preview first");
        store
            .commit_release_transition(&activate_first)
            .expect("activate first");
        let activate_second = store
            .preview_release_transition(ReleaseTransitionKind::Activate, second.release_id())
            .expect("preview second");
        store
            .commit_release_transition(&activate_second)
            .expect("activate second");
        assert_eq!(
            store
                .preview_release_transition(
                    ReleaseTransitionKind::Rollback,
                    never_active.release_id(),
                )
                .expect_err("never-active rollback")
                .code(),
            "release_rollback_target_not_active_history"
        );
        let rollback = store
            .preview_release_transition(ReleaseTransitionKind::Rollback, first.release_id())
            .expect("rollback preview");
        let active = store
            .commit_release_transition(&rollback)
            .expect("rollback");
        assert_eq!(active.manifest(), &first);
        assert_eq!(active.previous_release_id(), Some(second.release_id()));
        assert_eq!(store.release_transitions().expect("transitions").len(), 3);
    }

    #[test]
    fn release_staging_requires_complete_matching_artifacts() {
        let root = TempDir::new().expect("tempdir");
        let store = RuntimeStateStore::open(root.path(), b"0123456789abcdef").expect("store");
        let (manifest, sources) = release(root.path(), "1.0.0", 'a');
        fs::remove_file(sources.ui()).expect("remove UI source");
        let error = store
            .stage_release(manifest.clone(), &sources)
            .expect_err("missing source must fail");
        assert_eq!(error.code(), "release_artifact_source_invalid");
        assert!(error.is_fatal());
        assert!(store.release_generations().expect("generations").is_empty());

        fs::write(sources.ui(), b"tampered UI").expect("tampered UI source");
        let error = store
            .stage_release(manifest, &sources)
            .expect_err("hash mismatch must fail");
        assert_eq!(error.code(), "release_artifact_hash_mismatch");
        assert!(error.is_fatal());
        assert!(store.release_generations().expect("generations").is_empty());
    }

    #[test]
    fn release_identity_changes_when_same_version_has_different_bytes() {
        let root = TempDir::new().expect("tempdir");
        let (first, _) = release(root.path(), "1.0.0", 'a');
        let (second, _) = release(root.path(), "1.0.0", 'b');
        assert_eq!(first.runtime_version(), second.runtime_version());
        assert_eq!(first.ui_version(), second.ui_version());
        assert_ne!(first.release_id(), second.release_id());
    }

    #[test]
    fn active_release_resolves_only_verified_content_addressed_artifacts() {
        let root = TempDir::new().expect("tempdir");
        let store = RuntimeStateStore::open(root.path(), b"0123456789abcdef").expect("store");
        let (manifest, sources) = release(root.path(), "1.0.0", 'a');
        store
            .stage_release(manifest.clone(), &sources)
            .expect("stage release");
        let preview = store
            .preview_release_transition(ReleaseTransitionKind::Activate, manifest.release_id())
            .expect("preview activation");
        store
            .commit_release_transition(&preview)
            .expect("activate release");

        let runtime = store
            .resolve_active_release_artifact(&ReleaseArtifactKey::Runtime)
            .expect("resolve runtime artifact");
        assert_eq!(runtime.release_id(), manifest.release_id());
        assert_eq!(runtime.content_sha256(), manifest.runtime_content_sha256());
        assert_eq!(
            fs::read(runtime.path()).expect("runtime bytes"),
            b"runtime:1.0.0:a"
        );

        fs::write(runtime.path(), b"tampered runtime").expect("tamper runtime blob");
        let error = store
            .active_release()
            .expect_err("tampered active artifact must fail");
        assert_eq!(error.code(), "release_artifact_hash_mismatch");
        assert!(error.is_fatal());
    }

    #[test]
    fn release_activation_rejects_missing_staged_artifact() {
        let root = TempDir::new().expect("tempdir");
        let store = RuntimeStateStore::open(root.path(), b"0123456789abcdef").expect("store");
        let (manifest, sources) = release(root.path(), "1.0.0", 'a');
        store
            .stage_release(manifest.clone(), &sources)
            .expect("stage release");
        let runtime = store
            .release_blob_path(manifest.runtime_content_sha256(), "test")
            .expect("runtime blob path");
        fs::remove_file(runtime).expect("remove runtime blob");
        let error = store
            .preview_release_transition(ReleaseTransitionKind::Activate, manifest.release_id())
            .expect_err("missing artifact must fail");
        assert_eq!(error.code(), "release_artifact_unavailable");
        assert!(error.is_fatal());
    }

    #[test]
    fn stale_transition_preview_cannot_replace_a_new_pointer() {
        let root = TempDir::new().expect("tempdir");
        let store = RuntimeStateStore::open(root.path(), b"0123456789abcdef").expect("store");
        let (first, first_sources) = release(root.path(), "1.0.0", 'a');
        let (second, second_sources) = release(root.path(), "2.0.0", 'b');
        store
            .stage_release(first.clone(), &first_sources)
            .expect("stage first");
        store
            .stage_release(second.clone(), &second_sources)
            .expect("stage second");
        let stale = store
            .preview_release_transition(ReleaseTransitionKind::Activate, first.release_id())
            .expect("stale preview");
        let winner = store
            .preview_release_transition(ReleaseTransitionKind::Activate, second.release_id())
            .expect("winner preview");
        store
            .commit_release_transition(&winner)
            .expect("winner commit");
        assert_eq!(
            store
                .commit_release_transition(&stale)
                .expect_err("stale preview")
                .code(),
            "release_pointer_changed"
        );
    }

    #[test]
    fn persisted_integrity_key_survives_bootstrap_seed_rotation_and_missing_key_is_fatal() {
        let root = TempDir::new().expect("tempdir");
        let store = RuntimeStateStore::open(root.path(), b"0123456789abcdef").expect("store");
        store
            .write_json_document("policy.active", "pointer.v1", br#"{"value":1}"#, None)
            .expect("state document");
        drop(store);

        let reopened = RuntimeStateStore::open(root.path(), b"fedcba9876543210")
            .expect("reopen with rotated seed");
        assert!(
            reopened
                .read_json_document("policy.active")
                .expect("read document")
                .is_some()
        );
        drop(reopened);
        fs::remove_file(root.path().join(RUNTIME_STATE_INTEGRITY_KEY_FILE))
            .expect("remove integrity key");
        let error = RuntimeStateStore::open(root.path(), b"fedcba9876543210")
            .err()
            .expect("missing key must fail");
        assert_eq!(error.code(), "state_integrity_key_missing");
        assert!(error.is_fatal());
    }

    #[test]
    fn release_pointer_transaction_rolls_back_at_each_sqlite_write_boundary() {
        for (table, expected_code) in [
            (
                "release_pointer_history",
                "release_pointer_history_write_failed",
            ),
            ("release_pointer", "release_pointer_write_failed"),
            ("release_transitions", "release_transition_write_failed"),
        ] {
            let root = TempDir::new().expect("tempdir");
            let store = RuntimeStateStore::open(root.path(), b"0123456789abcdef").expect("store");
            let (first, first_sources) = release(root.path(), "1.0.0", 'a');
            let (second, second_sources) = release(root.path(), "2.0.0", 'b');
            store
                .stage_release(first.clone(), &first_sources)
                .expect("stage first");
            store
                .stage_release(second.clone(), &second_sources)
                .expect("stage second");
            let first_preview = store
                .preview_release_transition(ReleaseTransitionKind::Activate, first.release_id())
                .expect("first preview");
            store
                .commit_release_transition(&first_preview)
                .expect("first activation");
            let second_preview = store
                .preview_release_transition(ReleaseTransitionKind::Activate, second.release_id())
                .expect("second preview");
            {
                let connection = store
                    .connection("inject_release_fault")
                    .expect("connection");
                connection
                    .execute_batch(&format!(
                        "CREATE TEMP TRIGGER fail_release_write BEFORE INSERT ON {table} \
                         BEGIN SELECT RAISE(ABORT, 'injected'); END;"
                    ))
                    .expect("fault trigger");
            }
            let error = store
                .commit_release_transition(&second_preview)
                .expect_err("injected transition failure");
            assert_eq!(error.code(), expected_code);
            assert!(error.is_fatal());
            assert_eq!(
                store
                    .active_release()
                    .expect("active release")
                    .expect("active generation")
                    .manifest(),
                &first
            );
            assert_eq!(store.release_transitions().expect("transitions").len(), 1);
        }
    }

    #[test]
    fn committed_release_transition_replay_is_idempotent() {
        let root = TempDir::new().expect("tempdir");
        let store = RuntimeStateStore::open(root.path(), b"0123456789abcdef").expect("store");
        let (manifest, sources) = release(root.path(), "1.0.0", 'a');
        store
            .stage_release(manifest.clone(), &sources)
            .expect("stage release");
        let preview = store
            .preview_release_transition(ReleaseTransitionKind::Activate, manifest.release_id())
            .expect("preview");
        let first = store
            .commit_release_transition(&preview)
            .expect("first commit");
        let replay = store
            .commit_release_transition(&preview)
            .expect("replay commit");
        assert_eq!(first, replay);
        assert_eq!(store.release_transitions().expect("transitions").len(), 1);
    }

    #[test]
    fn concurrent_release_switches_commit_exactly_one_pointer_revision() {
        let root = TempDir::new().expect("tempdir");
        let store =
            Arc::new(RuntimeStateStore::open(root.path(), b"0123456789abcdef").expect("store"));
        let (first, first_sources) = release(root.path(), "1.0.0", 'a');
        let (second, second_sources) = release(root.path(), "2.0.0", 'b');
        store
            .stage_release(first.clone(), &first_sources)
            .expect("stage first");
        store
            .stage_release(second.clone(), &second_sources)
            .expect("stage second");
        let first_preview = store
            .preview_release_transition(ReleaseTransitionKind::Activate, first.release_id())
            .expect("first preview");
        let second_preview = store
            .preview_release_transition(ReleaseTransitionKind::Activate, second.release_id())
            .expect("second preview");
        let barrier = Arc::new(Barrier::new(3));
        let handles = [first_preview, second_preview]
            .into_iter()
            .map(|preview| {
                let store = Arc::clone(&store);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    store.commit_release_transition(&preview)
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let results = handles
            .into_iter()
            .map(|handle| handle.join().expect("release writer"))
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter_map(|result| result.as_ref().err())
                .map(RuntimeStateError::code)
                .collect::<Vec<_>>(),
            vec!["release_pointer_changed"]
        );
        assert_eq!(store.release_transitions().expect("transitions").len(), 1);
        assert_eq!(
            store
                .active_release()
                .expect("active release")
                .expect("active generation")
                .revision(),
            1
        );
    }

    #[test]
    fn executable_and_published_sqlite_schemas_are_identical() {
        let published = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("contracts")
            .join("sqlite")
            .join("schema.sql");
        assert_eq!(
            include_bytes!("schema.sql").as_slice(),
            fs::read(published).expect("published schema")
        );
    }

    #[test]
    fn state_document_rollback_creates_a_new_monotonic_revision() {
        let root = TempDir::new().expect("tempdir");
        let store = RuntimeStateStore::open(root.path(), b"0123456789abcdef").expect("store");
        let first = store
            .write_json_document("policy.active", "pointer.v1", br#"{"value":1}"#, None)
            .expect("first");
        let second = store
            .write_json_document(
                "policy.active",
                "pointer.v1",
                br#"{"value":2}"#,
                Some(first.payload_sha256()),
            )
            .expect("second");
        let third = store
            .write_json_document(
                "policy.active",
                "pointer.v2",
                br#"{"value":3}"#,
                Some(second.payload_sha256()),
            )
            .expect("third");
        let rollback = store
            .rollback_json_document("policy.active", 1, third.payload_sha256())
            .expect("rollback");
        assert_eq!(rollback.revision(), 4);
        assert_eq!(rollback.schema_version(), "pointer.v1");
        assert_eq!(rollback.payload(), first.payload());
        assert_eq!(
            rollback.previous_payload_sha256(),
            Some(third.payload_sha256())
        );
        drop(store);
        let reopened =
            RuntimeStateStore::open(root.path(), b"rotated-bootstrap-seed").expect("reopen store");
        assert_eq!(
            reopened
                .read_json_document("policy.active")
                .expect("read state")
                .expect("active state"),
            rollback
        );
    }
}
