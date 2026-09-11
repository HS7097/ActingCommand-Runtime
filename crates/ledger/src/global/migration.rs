// SPDX-License-Identifier: AGPL-3.0-only

use super::storage::LockedWriterFile;
use super::*;
use actingcommand_contract::{LedgerMigrationPhase, LedgerMigrationRecord, LedgerMigrationResult};
use actingcommand_runtime_database::{
    MaintenanceFile, MaintenanceLimits, RuntimeDatabase, digest, list_material,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerSourceIdentity {
    pub source_sha256: String,
    pub event_count: u64,
    pub first_sequence: u64,
    pub last_sequence: u64,
    pub head_sha256: String,
    pub content_sha256: String,
    pub files: Vec<MaintenanceFile>,
}

pub struct FrozenLedgerSource {
    pub(super) identity: LedgerSourceIdentity,
    pub(super) events: Vec<PersistedEvent>,
    pub(super) verified: Vec<(ProjectedArtifactReference, VerifiedArtifactReference)>,
}

impl FrozenLedgerSource {
    pub fn identity(&self) -> &LedgerSourceIdentity {
        &self.identity
    }
    pub fn artifacts(&self) -> Vec<ProjectedArtifactReference> {
        self.verified
            .iter()
            .map(|(reference, _)| reference.clone())
            .collect()
    }

    pub fn migration_record(
        &self,
        backup_sha256: &str,
        state_sha256: &str,
    ) -> GlobalLedgerResult<LedgerMigrationRecord> {
        let material = serde_json::to_vec(&(&self.identity, backup_sha256, state_sha256)).map_err(
            |error| {
                GlobalLedgerError::json(
                    "migration_identity_invalid",
                    "encode_migration_identity",
                    &error,
                )
            },
        )?;
        let record = LedgerMigrationRecord {
            migration_id: digest(&material).replacen("sha256:", "migration:", 1),
            source_sha256: self.identity.source_sha256.clone(),
            backup_sha256: backup_sha256.into(),
            source_event_count: self.identity.event_count,
            source_first_sequence: self.identity.first_sequence,
            source_last_sequence: self.identity.last_sequence,
            source_head_sha256: self.identity.head_sha256.clone(),
            imported_content_sha256: self.identity.content_sha256.clone(),
            state_sha256: state_sha256.into(),
            cutover_sequence: self
                .identity
                .last_sequence
                .checked_add(1)
                .ok_or_else(|| failure("sequence_exhausted", "prepare_migration"))?,
            phase: LedgerMigrationPhase::Cutover,
            result: LedgerMigrationResult::Committed,
        };
        record
            .validate()
            .map_err(|error| failure(error.code(), "validate_migration_record"))?;
        Ok(record)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum LedgerStorageStatus {
    Missing,
    Candidate,
    Ready {
        head_sequence: u64,
        head_sha256: Option<String>,
        migration: Option<Box<LedgerMigrationRecord>>,
    },
}

/// No journal writes occur until this same held file is moved into the final writer.
pub struct LedgerMaintenance {
    root: PathBuf,
    lock: LockedWriterFile,
    compatibility: Option<LockedWriterFile>,
    limits: MaintenanceLimits,
    deadline: Instant,
}

impl LedgerMaintenance {
    pub fn acquire(
        root: &Path,
        allow_new: bool,
        limits: MaintenanceLimits,
        deadline: Instant,
    ) -> GlobalLedgerResult<Self> {
        limits.check(0, 0, deadline)?;
        let root = root.canonicalize().map_err(|error| {
            GlobalLedgerError::io("ledger_io", "resolve_maintenance_root", &error)
        })?;
        let compatibility_path = root.join("writer.lock");
        let compatibility = match fs::symlink_metadata(&compatibility_path) {
            Ok(_) => {
                let lock = LockedWriterFile::open(&compatibility_path, false)?;
                lock.require_closed()?;
                Some(lock)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(GlobalLedgerError::io(
                    "ledger_io",
                    "inspect_candidate_writer_lock",
                    &error,
                ));
            }
        };
        let ledger_root = root.join("ledger");
        if allow_new {
            fs::create_dir_all(&ledger_root).map_err(|error| {
                GlobalLedgerError::io("ledger_io", "create_runtime_ledger_root", &error)
            })?;
        }
        let lock = LockedWriterFile::open(&ledger_root.join("writer.lock"), allow_new)?;
        Ok(Self {
            root,
            lock,
            compatibility,
            limits,
            deadline,
        })
    }

    pub fn source<F>(&self, mut verifier: F) -> GlobalLedgerResult<FrozenLedgerSource>
    where
        F: FnMut(&ProjectedArtifactReference) -> Option<VerifiedArtifactReference>,
    {
        self.lock.require_closed()?;
        let before = self.source_files()?;
        let mut verified = Vec::new();
        let snapshot = GlobalLedger::open_read_only(
            GlobalLedgerReadOnlyConfig::new(self.root.join("ledger")).with_budget(
                self.limits.max_bytes,
                self.limits.max_events,
                self.deadline,
            ),
            |reference| {
                let result = verifier(reference)?;
                verified.push((reference.clone(), result.clone()));
                Some(result)
            },
        )?;
        let physical = snapshot.storage_snapshot();
        if !physical.read_complete
            || physical.read_bytes != physical.observed_bytes
            || physical.verified_prefix_bytes != physical.observed_bytes
            || snapshot.corrupt_tail().is_some()
            || snapshot.repairs().iter().any(|repair| !repair.completed())
        {
            return Err(failure(
                "migration_source_incomplete",
                "validate_segment_import_source",
            ));
        }
        // The native reader's physical atomic=false remains unchanged. These locks and
        // the frozen material identity are the offline migration's consistency boundary.
        storage::verify_completed_repairs(&self.root.join("ledger"), snapshot.events())?;
        let after = self.source_files()?;
        if before != after {
            return Err(failure(
                "migration_source_changed",
                "freeze_segment_import_source",
            ));
        }
        let events = snapshot.events().to_vec();
        let content = canonical_digest(&events)?;
        let head = events
            .last()
            .map(canonical_record)
            .transpose()?
            .map_or_else(|| digest(&[]), |bytes| digest(&bytes));
        let identity = LedgerSourceIdentity {
            source_sha256: digest(&serde_json::to_vec(&before).map_err(|error| {
                GlobalLedgerError::json(
                    "migration_source_invalid",
                    "encode_source_material",
                    &error,
                )
            })?),
            event_count: events.len() as u64,
            first_sequence: events.first().map_or(0, PersistedEvent::sequence),
            last_sequence: events.last().map_or(0, PersistedEvent::sequence),
            head_sha256: head,
            content_sha256: content,
            files: before,
        };
        Ok(FrozenLedgerSource {
            identity,
            events,
            verified,
        })
    }

    pub fn source_files(&self) -> GlobalLedgerResult<Vec<MaintenanceFile>> {
        Ok(list_material(
            &self.root,
            "ledger",
            &["ledger/writer.lock"],
            self.limits,
            self.deadline,
        )?
        .into_iter()
        .filter(|file| file.relative_path != "ledger/writer.lock")
        .collect())
    }

    /// Copies metadata through the already locked handles, without changing the source journals.
    pub fn locked_material(&self) -> GlobalLedgerResult<Vec<(String, Vec<u8>)>> {
        let mut material = vec![("ledger/writer.lock".into(), self.lock.bytes()?)];
        if let Some(lock) = &self.compatibility {
            material.push(("writer.lock".into(), lock.bytes()?));
        }
        Ok(material)
    }

    pub fn read_formal<F>(
        &self,
        database: &RuntimeDatabase,
        verifier: F,
    ) -> GlobalLedgerResult<SqliteLedgerReadOnly>
    where
        F: FnMut(&ProjectedArtifactReference) -> Option<VerifiedArtifactReference>,
    {
        self.validate_database(database)?;
        sqlite::SqliteLedgerReadOnly::open_formal(database, self.budget(), verifier)
    }

    pub fn status<F>(
        &self,
        database: &RuntimeDatabase,
        mut verifier: F,
    ) -> GlobalLedgerResult<LedgerStorageStatus>
    where
        F: FnMut(&ProjectedArtifactReference) -> Option<VerifiedArtifactReference>,
    {
        self.validate_database(database)?;
        sqlite::storage_status(database, &mut verifier, self.budget())
    }

    pub fn import(
        &self,
        database: &RuntimeDatabase,
        source: &FrozenLedgerSource,
        record: &LedgerMigrationRecord,
        completion: SanitizedEventDraft,
        dry_run: bool,
    ) -> GlobalLedgerResult<LedgerStorageStatus> {
        self.validate_database(database)?;
        self.lock.require_closed()?;
        if self.source_files()? != source.identity.files {
            return Err(failure("migration_source_changed", "import_segment_ledger"));
        }
        let expected = source.migration_record(&record.backup_sha256, &record.state_sha256)?;
        if expected != *record {
            return Err(failure(
                "migration_binding_mismatch",
                "import_segment_ledger",
            ));
        }
        sqlite::import_source(database, source, record, completion, dry_run, self.budget())
    }

    pub fn initialize_empty(&self, database: &RuntimeDatabase) -> GlobalLedgerResult<()> {
        self.validate_database(database)?;
        if !self.lock.newly_created()
            || self.compatibility.is_some()
            || !self.source_files()?.is_empty()
        {
            return Err(failure(
                "ledger_not_an_empty_root",
                "initialize_runtime_ledger",
            ));
        }
        sqlite::initialize_formal_empty(database)
    }

    pub fn open_writer<F>(
        self,
        database: Arc<RuntimeDatabase>,
        owner_id: String,
        verifier: F,
    ) -> GlobalLedgerResult<GlobalLedger>
    where
        F: FnMut(&ProjectedArtifactReference) -> Option<VerifiedArtifactReference>,
    {
        self.validate_database(&database)?;
        let config = GlobalLedgerConfig::new(self.root.join("ledger"), owner_id);
        GlobalLedger::open_with_store(config, move |config| {
            sqlite::open_formal(config, database, self.lock, self.compatibility, verifier)
        })
    }

    pub fn close(self) -> GlobalLedgerResult<()> {
        let result = self.lock.close();
        let compatibility = self.compatibility.map_or(Ok(()), LockedWriterFile::close);
        match result {
            Ok(()) => compatibility,
            Err(error) => Err(error.with_close_result(compatibility)),
        }
    }

    fn budget(&self) -> Option<(u64, usize, Instant)> {
        Some((self.limits.max_bytes, self.limits.max_events, self.deadline))
    }
    fn validate_database(&self, database: &RuntimeDatabase) -> GlobalLedgerResult<()> {
        if database
            .root()
            .canonicalize()
            .map_err(|error| GlobalLedgerError::io("ledger_io", "resolve_database_root", &error))?
            != self.root
        {
            return Err(failure("ledger_database_root_mismatch", "maintain_ledger"));
        }
        self.limits.check(0, 0, self.deadline)?;
        Ok(())
    }
}

pub(super) fn canonical_record(event: &PersistedEvent) -> GlobalLedgerResult<Vec<u8>> {
    canonical_stored_record(&crate::fact::StoredEventRecord::from_event(event))
}
pub(super) fn canonical_stored_record(
    record: &crate::fact::StoredEventRecord,
) -> GlobalLedgerResult<Vec<u8>> {
    serde_json::to_vec(record).map_err(|error| {
        GlobalLedgerError::json("migration_record_invalid", "encode_import_record", &error)
    })
}
pub(super) fn canonical_digest(events: &[PersistedEvent]) -> GlobalLedgerResult<String> {
    canonical_stored_digest(
        &events
            .iter()
            .map(crate::fact::StoredEventRecord::from_event)
            .collect::<Vec<_>>(),
    )
}
pub(super) fn canonical_stored_digest(
    events: &[crate::fact::StoredEventRecord],
) -> GlobalLedgerResult<String> {
    let mut hash = Sha256::new();
    for event in events {
        let bytes = canonical_stored_record(event)?;
        hash.update((bytes.len() as u64).to_be_bytes());
        hash.update(bytes);
    }
    Ok(format!("sha256:{:x}", hash.finalize()))
}
fn failure(code: &'static str, operation: &'static str) -> GlobalLedgerError {
    GlobalLedgerError::fatal(code, operation)
}
