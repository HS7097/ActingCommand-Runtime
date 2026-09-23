// SPDX-License-Identifier: AGPL-3.0-only

//! Offline `actingd unlock-owner` (`contracts/actingd-unlock-owner.md`).

use crate::events::RuntimeEvents;
use crate::owner::{OwnerUnlock, unlock_retained_owner};
use crate::{RuntimeClock, RuntimeHostError, RuntimeHostResult};
use actingcommand_artifact_store::ArtifactStore;
use actingcommand_contract::{
    AuditInput, ClientPayloadDraft, EventActor, EventSeverity, EventSource, OriginModule,
    OwnerEpoch, OwnerResourceDisposition, OwnerUnlockActor, RuntimeErrorCode,
};
use actingcommand_ledger::{GlobalLedgerError, LedgerMaintenance, LedgerStorageStatus};
use actingcommand_runtime_database::{MaintenanceLimits, RuntimeDatabase, RuntimeDatabaseError};
use std::path::Path;
use std::sync::Arc;

/// The retained epoch the operator unlocked and the owner journal revision that did it.
#[derive(Debug, Clone)]
pub struct OwnerUnlockReceipt {
    pub owner_epoch: OwnerEpoch,
    pub previous_resource_disposition: OwnerResourceDisposition,
    pub revision: u64,
}

#[derive(Debug, Clone)]
pub struct OwnerUnlockFailure {
    pub error: RuntimeHostError,
    /// `journal` up to and including the owner journal append, `ledger` for the fact.
    pub stage: &'static str,
    /// The unlock record is durable; the next start takes the epoch over regardless.
    pub journal_appended: bool,
}

pub(crate) fn run(
    root: &Path,
    salt: &[u8],
    clock: Arc<dyn RuntimeClock>,
    actor: OwnerUnlockActor,
    confirmed: bool,
) -> Result<OwnerUnlockReceipt, OwnerUnlockFailure> {
    let unlocked = unlock_retained_owner(root, confirmed).map_err(|error| OwnerUnlockFailure {
        error,
        stage: "journal",
        journal_appended: false,
    })?;
    // The fact is recorded under the still-held owner lock. No new epoch is acquired, so
    // the confirmed epoch remains the last record and the next start takes it over.
    record(root, salt, clock, actor, &unlocked).map_err(|error| OwnerUnlockFailure {
        error,
        stage: "ledger",
        journal_appended: true,
    })?;
    Ok(OwnerUnlockReceipt {
        owner_epoch: unlocked.owner_epoch,
        previous_resource_disposition: unlocked.previous_resource_disposition,
        revision: unlocked.revision,
    })
}

fn record(
    root: &Path,
    salt: &[u8],
    clock: Arc<dyn RuntimeClock>,
    actor: OwnerUnlockActor,
    unlocked: &OwnerUnlock,
) -> RuntimeHostResult<()> {
    let events = RuntimeEvents::new(salt, Arc::clone(&clock))?;
    let limits = MaintenanceLimits::default();
    let deadline = limits.deadline().map_err(database_error)?;
    // Existing storage only: the retained owner already ran on this root.
    let database = Arc::new(RuntimeDatabase::open_existing(root, false).map_err(database_error)?);
    let artifacts = ArtifactStore::open(root).map_err(RuntimeHostError::artifact)?;
    let maintenance =
        LedgerMaintenance::acquire(root, false, limits, deadline).map_err(ledger_error)?;
    let status = maintenance
        .status(&database, |reference| {
            artifacts.verify_recovery_reference(reference).ok()
        })
        .map_err(ledger_error)?;
    if !matches!(status, LedgerStorageStatus::Ready { .. }) {
        return Err(RuntimeHostError::fatal(
            "ledger_migration_required",
            "select_runtime_storage",
            RuntimeErrorCode::LedgerFailure,
        ));
    }
    let owner_id = format!(
        "actingd-unlock-owner-{}-{}",
        std::process::id(),
        clock.sample()?.unix_ms
    );
    let ledger = maintenance
        .open_writer(database, owner_id, |reference| {
            artifacts.verify_recovery_reference(reference).ok()
        })
        .map_err(ledger_error)?;
    let appended = events
        .system_links()
        .and_then(|links| {
            events.draft(
                EventSeverity::Info,
                EventSource::Cli,
                OriginModule::Runtime,
                EventActor::User,
                links,
                ClientPayloadDraft::owner_unlock(
                    unlocked.owner_epoch,
                    unlocked.previous_resource_disposition,
                    actor,
                    AuditInput::new(),
                ),
            )
        })
        .and_then(|draft| events.sanitize(draft))
        .and_then(|draft| ledger.append(draft).map(drop).map_err(ledger_error));
    match (appended, ledger.close().map_err(ledger_error)) {
        (Err(error), Err(closed)) => {
            Err(error.with_related_failure("close_unlock_ledger", &closed))
        }
        (appended, closed) => appended.and(closed),
    }
}

fn ledger_error(error: GlobalLedgerError) -> RuntimeHostError {
    RuntimeHostError::fatal(
        error.code(),
        error.operation(),
        RuntimeErrorCode::LedgerFailure,
    )
    .with_native_detail(format!("{error:?}"))
}

fn database_error(error: RuntimeDatabaseError) -> RuntimeHostError {
    RuntimeHostError::fatal(
        error.code(),
        error.operation(),
        RuntimeErrorCode::LedgerFailure,
    )
    .with_native_detail(format!("{error:?}"))
}
