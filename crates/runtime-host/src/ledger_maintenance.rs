// SPDX-License-Identifier: AGPL-3.0-only

use crate::events::RuntimeEvents;
use crate::owner::{OwnerGuard, OwnerStartup};
use crate::{RuntimeClock, RuntimeHostError};
use actingcommand_artifact_store::{ArtifactStore, ArtifactStoreError};
use actingcommand_contract::{
    AuditInput, EventActor, EventSeverity, EventSource, LedgerPayloadDraft, OriginModule,
    ProjectedArtifactReference,
};
use actingcommand_ledger::{
    GlobalLedgerError, LedgerMaintenance, LedgerSourceIdentity, LedgerStorageStatus,
};
use actingcommand_runtime_database::{
    DatabaseBackup, MaintenanceLimits, RuntimeDatabase, RuntimeDatabaseError, list_material,
    require_disjoint,
};
use actingcommand_runtime_state::{RuntimeStateError, RuntimeStateStore};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LedgerMaintenanceOperation {
    DryRun,
    Import,
    Verify,
    Backup,
    Restore,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerMaintenanceRequest {
    pub operation: LedgerMaintenanceOperation,
    pub backup: Option<PathBuf>,
    pub target: Option<PathBuf>,
    pub artifact_root: Option<PathBuf>,
    #[serde(default)]
    pub limits: MaintenanceLimits,
}

#[derive(Debug, Clone, Serialize)]
pub struct LedgerMaintenanceReceipt {
    pub schema_version: &'static str,
    pub status: &'static str,
    pub ledger: LedgerStorageStatus,
    pub backup_id: Option<String>,
    pub warnings: Vec<actingcommand_runtime_database::MaintenanceWarning>,
    pub activated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct LedgerMaintenanceFailure {
    pub code: String,
    pub operation: String,
    pub cause: String,
    pub cleanup: Vec<String>,
    pub warnings: Vec<actingcommand_runtime_database::MaintenanceWarning>,
}

impl std::fmt::Display for LedgerMaintenanceFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.cause)?;
        for error in &self.cleanup {
            write!(f, "; cleanup: {error}")?;
        }
        Ok(())
    }
}
impl std::error::Error for LedgerMaintenanceFailure {}
type Result<T> = std::result::Result<T, LedgerMaintenanceFailure>;

macro_rules! maintenance_error {
    ($kind:ty) => {
        impl From<$kind> for LedgerMaintenanceFailure {
            fn from(error: $kind) -> Self {
                Self {
                    code: error.code().into(),
                    operation: error.operation().into(),
                    cause: error.to_string(),
                    cleanup: Vec::new(),
                    warnings: Vec::new(),
                }
            }
        }
    };
}
maintenance_error!(RuntimeHostError);
impl From<GlobalLedgerError> for LedgerMaintenanceFailure {
    fn from(error: GlobalLedgerError) -> Self {
        Self {
            code: error.code().into(),
            operation: error.operation().into(),
            cause: format!(
                "{error}; {}",
                error.detail().unwrap_or("no_additional_detail")
            ),
            cleanup: Vec::new(),
            warnings: Vec::new(),
        }
    }
}
impl From<RuntimeDatabaseError> for LedgerMaintenanceFailure {
    fn from(error: RuntimeDatabaseError) -> Self {
        Self {
            code: error.code().into(),
            operation: error.operation().into(),
            cause: error.to_string(),
            cleanup: Vec::new(),
            warnings: error.warnings().to_vec(),
        }
    }
}
maintenance_error!(RuntimeStateError);
maintenance_error!(ArtifactStoreError);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BackupBinding {
    ledger: LedgerStorageStatus,
    state_sha256: String,
    source: Option<LedgerSourceIdentity>,
    artifacts: Vec<ProjectedArtifactReference>,
}

pub(crate) fn run(
    root: &Path,
    salt: &[u8],
    clock: Arc<dyn RuntimeClock>,
    mut request: LedgerMaintenanceRequest,
) -> Result<LedgerMaintenanceReceipt> {
    validate_request(&request)?;
    for path in [&mut request.backup, &mut request.target]
        .into_iter()
        .flatten()
    {
        *path = actingcommand_runtime_database::resolve_maintenance_destination(path)?;
    }
    let deadline = request.limits.deadline()?;
    let events = RuntimeEvents::new(salt, Arc::clone(&clock))?;
    let OwnerStartup {
        guard: mut owner, ..
    } = OwnerGuard::acquire(root, events.issuer(), clock.sample()?.unix_ms)?;
    let result = (|| {
        let maintenance = LedgerMaintenance::acquire(root, false, request.limits, deadline)?;
        let result = run_locked(root, &events, &maintenance, &request, deadline);
        finish(result, maintenance.close().map_err(Into::into))
    })();
    let closed = clock
        .sample()
        .map_err(Into::into)
        .and_then(|sample| owner.close(sample.unix_ms).map_err(Into::into));
    finish(result, closed)
}

fn validate_request(request: &LedgerMaintenanceRequest) -> Result<()> {
    let needs_backup = request.operation != LedgerMaintenanceOperation::Verify;
    if needs_backup != request.backup.is_some()
        || (request.operation == LedgerMaintenanceOperation::Restore) != request.target.is_some()
        || request.artifact_root.is_some()
            && request.operation != LedgerMaintenanceOperation::Restore
    {
        return Err(failure(
            "maintenance_arguments_invalid",
            "validate_ledger_maintenance",
        ));
    }
    Ok(())
}

fn run_locked(
    root: &Path,
    events: &RuntimeEvents,
    maintenance: &LedgerMaintenance,
    request: &LedgerMaintenanceRequest,
    deadline: Instant,
) -> Result<LedgerMaintenanceReceipt> {
    // Existing-only: maintenance cannot bootstrap a missing State database or key.
    let database = Arc::new(RuntimeDatabase::open_existing(
        root,
        request.operation == LedgerMaintenanceOperation::Verify
            || request.operation == LedgerMaintenanceOperation::Backup
            || request.operation == LedgerMaintenanceOperation::Restore,
    )?);
    let state = RuntimeStateStore::from_database(Arc::clone(&database))?;
    let state_sha256 = state.maintenance_digest(request.limits, deadline)?;
    let artifacts = ArtifactStore::open(root)?;
    // Marker lookup precedes source import and any backup decision.
    let ledger = maintenance.status(&database, |reference| {
        artifacts.verify_recovery_reference(reference).ok()
    })?;
    if ledger == LedgerStorageStatus::Candidate {
        return Err(failure(
            "candidate_requires_explicit_disposition",
            "maintain_runtime_ledger",
        ));
    }
    match request.operation {
        LedgerMaintenanceOperation::Verify => {
            if ledger == LedgerStorageStatus::Missing {
                maintenance
                    .source(|reference| artifacts.verify_recovery_reference(reference).ok())?;
            }
            Ok(receipt(
                if ledger == LedgerStorageStatus::Missing {
                    "verified-segment"
                } else {
                    "verified-sqlite"
                },
                ledger,
                None,
            ))
        }
        LedgerMaintenanceOperation::Backup => {
            let binding = binding(
                &database,
                maintenance,
                &artifacts,
                ledger.clone(),
                state_sha256,
            )?;
            let mut material = maintenance.source_files()?;
            material.extend(list_material(
                root,
                "release-blobs",
                &[],
                request.limits,
                deadline,
            )?);
            let destination = required(&request.backup)?;
            let backup = database.backup(
                destination,
                &material,
                &maintenance.locked_material()?,
                serde_json::to_value(&binding)
                    .map_err(|_| failure("backup_binding_invalid", "encode_backup_binding"))?,
                request.limits,
                deadline,
            )?;
            verify_backup_binding(
                &database,
                destination,
                root,
                &backup,
                request.limits,
                deadline,
            )?;
            Ok(receipt("backed-up", ledger, Some(&backup)))
        }
        LedgerMaintenanceOperation::DryRun | LedgerMaintenanceOperation::Import => {
            let backup_path = required(&request.backup)?;
            // A frozen backup is an explicit input, including the first import.
            let backup = database.verify_backup(backup_path, request.limits, deadline)?;
            let binding = verify_backup_binding(
                &database,
                backup_path,
                root,
                &backup,
                request.limits,
                deadline,
            )?;
            let source = maintenance
                .source(|reference| artifacts.verify_recovery_reference(reference).ok())?;
            if binding.ledger != LedgerStorageStatus::Missing
                || binding.source.as_ref() != Some(source.identity())
            {
                return Err(failure(
                    "migration_backup_source_mismatch",
                    "import_segment_ledger",
                ));
            }
            let record = source.migration_record(&backup.backup_id, &binding.state_sha256)?;
            match &ledger {
                LedgerStorageStatus::Ready {
                    migration: Some(existing),
                    ..
                } if existing.as_ref() == &record => {
                    return Ok(receipt("already-imported", ledger, Some(&backup)));
                }
                LedgerStorageStatus::Missing if state_sha256 == binding.state_sha256 => {}
                _ => {
                    return Err(failure(
                        "migration_current_state_conflict",
                        "import_segment_ledger",
                    ));
                }
            }
            let completion = events.sanitize(events.draft(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::GlobalLedger,
                EventActor::System,
                events.system_links()?,
                LedgerPayloadDraft::migrated(record.clone(), AuditInput::new()),
            )?)?;
            let imported = maintenance.import(
                &database,
                &source,
                &record,
                completion,
                request.operation == LedgerMaintenanceOperation::DryRun,
            )?;
            Ok(receipt(
                if request.operation == LedgerMaintenanceOperation::DryRun {
                    "dry-run"
                } else {
                    "imported"
                },
                if request.operation == LedgerMaintenanceOperation::DryRun {
                    ledger
                } else {
                    imported
                },
                Some(&backup),
            ))
        }
        LedgerMaintenanceOperation::Restore => restore(
            root,
            events,
            &database,
            maintenance,
            (ledger, state_sha256),
            request,
            deadline,
        ),
    }
}

fn binding(
    database: &Arc<RuntimeDatabase>,
    maintenance: &LedgerMaintenance,
    artifacts: &ArtifactStore,
    ledger: LedgerStorageStatus,
    state_sha256: String,
) -> Result<BackupBinding> {
    let (source, references) = if ledger == LedgerStorageStatus::Missing {
        let source =
            maintenance.source(|reference| artifacts.verify_recovery_reference(reference).ok())?;
        (Some(source.identity().clone()), source.artifacts())
    } else {
        let snapshot = maintenance.read_formal(database, |reference| {
            artifacts.verify_recovery_reference(reference).ok()
        })?;
        (
            None,
            snapshot
                .events()
                .iter()
                .flat_map(|event| {
                    event
                        .artifacts()
                        .iter()
                        .map(|reference| reference.project(true))
                })
                .collect(),
        )
    };
    Ok(BackupBinding {
        ledger,
        state_sha256,
        source,
        artifacts: references,
    })
}

fn verify_backup_binding(
    database: &RuntimeDatabase,
    backup_root: &Path,
    artifact_root: &Path,
    backup: &DatabaseBackup,
    limits: MaintenanceLimits,
    deadline: Instant,
) -> Result<BackupBinding> {
    let verified = database.verify_backup(backup_root, limits, deadline)?;
    if verified != *backup {
        return Err(failure("backup_changed", "verify_backup_binding"));
    }
    let expected: BackupBinding = serde_json::from_value(backup.binding.clone())
        .map_err(|_| failure("backup_binding_invalid", "decode_backup_binding"))?;
    let archived_database = Arc::new(RuntimeDatabase::open_existing(backup_root, true)?);
    let state = RuntimeStateStore::from_database(Arc::clone(&archived_database))?;
    let state_sha256 = state.maintenance_digest(limits, deadline)?;
    let artifacts = ArtifactStore::open(artifact_root)?;
    let archived = LedgerMaintenance::acquire(backup_root, false, limits, deadline)?;
    let result = (|| {
        let ledger = archived.status(&archived_database, |reference| {
            artifacts.verify_recovery_reference(reference).ok()
        })?;
        let actual = binding(
            &archived_database,
            &archived,
            &artifacts,
            ledger,
            state_sha256,
        )?;
        if actual != expected {
            return Err(failure(
                "backup_projection_mismatch",
                "verify_backup_binding",
            ));
        }
        Ok(actual)
    })();
    finish(result, archived.close().map_err(Into::into))
}

fn restore(
    root: &Path,
    events: &RuntimeEvents,
    database: &RuntimeDatabase,
    maintenance: &LedgerMaintenance,
    current: (LedgerStorageStatus, String),
    request: &LedgerMaintenanceRequest,
    deadline: Instant,
) -> Result<LedgerMaintenanceReceipt> {
    let (current, current_state) = current;
    let backup_root = required(&request.backup)?;
    let target = required(&request.target)?;
    require_disjoint(root, target)?;
    require_disjoint(backup_root, target)?;
    let backup = database.verify_backup(backup_root, request.limits, deadline)?;
    let artifact_root = request.artifact_root.as_deref().unwrap_or(root);
    let expected = verify_backup_binding(
        database,
        backup_root,
        artifact_root,
        &backup,
        request.limits,
        deadline,
    )?;
    if current_state != expected.state_sha256 {
        return Err(failure(
            "restore_state_has_advanced",
            "admit_database_restore",
        ));
    }
    match (&expected.ledger, &current) {
        (LedgerStorageStatus::Missing, LedgerStorageStatus::Missing) => {
            let artifacts = ArtifactStore::open(root)?;
            let source = maintenance
                .source(|reference| artifacts.verify_recovery_reference(reference).ok())?;
            if expected.source.as_ref() != Some(source.identity()) {
                return Err(failure("restore_source_changed", "admit_database_restore"));
            }
        }
        (
            LedgerStorageStatus::Missing,
            LedgerStorageStatus::Ready {
                head_sequence,
                migration: Some(record),
                ..
            },
        ) if *head_sequence == record.cutover_sequence
            && record.backup_sha256 == backup.backup_id
            && expected
                .source
                .as_ref()
                .is_some_and(|source| source.source_sha256 == record.source_sha256) => {}
        (expected, actual) if expected == actual => {}
        _ => {
            return Err(failure(
                "restore_would_discard_new_events",
                "admit_database_restore",
            ));
        }
    }
    if target.exists() {
        return Err(failure("restore_target_exists", "admit_database_restore"));
    }
    std::fs::create_dir(target).map_err(|error| io("create_restore_target", &error))?;
    let OwnerStartup {
        guard: mut target_owner,
        ..
    } = OwnerGuard::acquire(target, events.issuer(), now()?)?;
    let result = (|| {
        database.restore_backup(backup_root, target, request.limits, deadline)?;
        let target_artifacts = ArtifactStore::open(target)?;
        let mut artifact_bytes = 0_u64;
        let mut seen = std::collections::BTreeSet::new();
        for reference in &expected.artifacts {
            if !seen.insert(reference.object_key().map(str::to_owned)) {
                continue;
            }
            artifact_bytes = artifact_bytes
                .checked_add(reference.byte_count)
                .ok_or_else(|| {
                    failure(
                        "restore_artifact_size_overflow",
                        "restore_artifact_material",
                    )
                })?;
            request.limits.check(artifact_bytes, seen.len(), deadline)?;
            target_artifacts.restore_recovery_reference(
                artifact_root,
                reference,
                request.limits.max_bytes,
                deadline,
            )?;
        }
        let restored_db = Arc::new(RuntimeDatabase::open_existing(target, true)?);
        let restored_state = RuntimeStateStore::from_database(Arc::clone(&restored_db))?;
        if restored_state.maintenance_digest(request.limits, deadline)? != expected.state_sha256 {
            return Err(failure("restored_state_mismatch", "verify_restored_root"));
        }
        let restored = LedgerMaintenance::acquire(target, false, request.limits, deadline)?;
        let verified = (|| {
            let ledger = restored.status(&restored_db, |reference| {
                target_artifacts.verify_recovery_reference(reference).ok()
            })?;
            let actual = binding(
                &restored_db,
                &restored,
                &target_artifacts,
                ledger,
                expected.state_sha256.clone(),
            )?;
            if actual != expected {
                return Err(failure(
                    "restored_projection_mismatch",
                    "verify_restored_root",
                ));
            }
            Ok(receipt("restored", actual.ledger, Some(&backup)))
        })();
        finish(verified, restored.close().map_err(Into::into))
    })();
    finish(
        result,
        now().and_then(|time| target_owner.close(time).map_err(Into::into)),
    )
}

fn receipt(
    status: &'static str,
    ledger: LedgerStorageStatus,
    backup: Option<&DatabaseBackup>,
) -> LedgerMaintenanceReceipt {
    LedgerMaintenanceReceipt {
        schema_version: "actingcommand.ledger-maintenance.v1",
        status,
        ledger,
        backup_id: backup.map(|backup| backup.backup_id.clone()),
        warnings: backup.map_or_else(Vec::new, |backup| backup.warnings.clone()),
        activated: false,
    }
}
fn required(path: &Option<PathBuf>) -> Result<&Path> {
    path.as_deref()
        .ok_or_else(|| failure("maintenance_path_missing", "validate_ledger_maintenance"))
}
fn failure(code: &str, operation: &str) -> LedgerMaintenanceFailure {
    LedgerMaintenanceFailure {
        code: code.into(),
        operation: operation.into(),
        cause: format!("{code} during {operation}"),
        cleanup: Vec::new(),
        warnings: Vec::new(),
    }
}
fn io(operation: &str, error: &std::io::Error) -> LedgerMaintenanceFailure {
    let mut result = failure("maintenance_io_failed", operation);
    result.cause.push_str(&format!(
        ": kind={:?};os={:?}",
        error.kind(),
        error.raw_os_error()
    ));
    result
}
fn now() -> Result<u64> {
    crate::SystemRuntimeClock::new()
        .sample()
        .map(|sample| sample.unix_ms)
        .map_err(Into::into)
}
fn finish<T>(result: Result<T>, cleanup: Result<()>) -> Result<T> {
    match (result, cleanup) {
        (result, Ok(())) => result,
        (Ok(_), Err(error)) => Err(error),
        (Err(mut original), Err(error)) => {
            original.cleanup.push(error.to_string());
            Err(original)
        }
    }
}
