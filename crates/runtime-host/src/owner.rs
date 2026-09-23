// SPDX-License-Identifier: AGPL-3.0-only

use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::{
    IdentifierIssuer, InstanceId, OwnerEpoch, OwnerResourceDisposition, RuntimeErrorCode,
};
use actingcommand_ledger::owner_journal::{RuntimeOwnerJournal, RuntimeOwnerRecord as OwnerRecord};
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;
use std::process;

pub(crate) const OWNER_FILE_NAME: &str = "owner.lock";
const OWNER_SCHEMA_VERSION: &str = actingcommand_contract::OWNER_JOURNAL_SCHEMA;

pub(crate) struct OwnerStartup {
    pub(crate) guard: OwnerGuard,
    pub(crate) owner_epoch: OwnerEpoch,
    pub(crate) takeover_instances: Vec<InstanceId>,
    pub(crate) takeover: bool,
    pub(crate) journal: RuntimeOwnerJournal,
}

pub(crate) struct OwnerGuard {
    file: Option<File>,
    retained_file: Option<&'static mut File>,
    record: OwnerRecord,
    closed: bool,
    retained_unconfirmed: bool,
    retention_result: Option<RuntimeHostResult<()>>,
    close_result: Option<RuntimeHostResult<()>>,
}

impl OwnerGuard {
    pub(crate) fn acquire(
        state_root: &Path,
        issuer: &IdentifierIssuer,
        started_at_unix_ms: u64,
    ) -> RuntimeHostResult<OwnerStartup> {
        let path = state_root.join(OWNER_FILE_NAME);
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .map_err(|_| {
                RuntimeHostError::fatal(
                    "owner_file_open_failed",
                    "open_owner_file",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
        try_lock_owner_file(&file, "owner_conflict", "acquire_owner_file")?;
        let journal = read_owner_journal(&mut file)?;
        let previous = journal.last().cloned();
        if previous.as_ref().is_some_and(|record| {
            record.schema_version == OWNER_SCHEMA_VERSION
                && matches!(
                    record.resource_disposition,
                    Some(OwnerResourceDisposition::InUse)
                        | Some(OwnerResourceDisposition::Unconfirmed)
                )
        }) {
            return Err(RuntimeHostError::fatal(
                "owner_resource_unconfirmed",
                "acquire_owner_file",
                RuntimeErrorCode::OwnerConflict,
            ));
        }
        let owner_epoch = *issuer
            .mint_owner_epoch()
            .map_err(|_| {
                RuntimeHostError::fatal(
                    "owner_epoch_issue_failed",
                    "acquire_owner_file",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?
            .transport();
        let takeover = previous.as_ref().is_some_and(|record| record.active);
        let takeover_instances = previous
            .as_ref()
            .filter(|record| record.active)
            .map(|record| record.active_instances.clone())
            .unwrap_or_default();
        let revision = previous
            .map(|record| record.revision)
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| {
                RuntimeHostError::fatal(
                    "owner_revision_overflow",
                    "acquire_owner_file",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
        let record = OwnerRecord {
            schema_version: OWNER_SCHEMA_VERSION.to_string(),
            revision,
            owner_epoch,
            pid: process::id(),
            started_at_unix_ms,
            active: true,
            active_instances: takeover_instances.clone(),
            closed_at_unix_ms: None,
            resource_disposition: Some(OwnerResourceDisposition::None),
        };
        append_record(&mut file, &record)?;
        Ok(OwnerStartup {
            guard: OwnerGuard {
                file: Some(file),
                retained_file: None,
                record,
                closed: false,
                retained_unconfirmed: false,
                retention_result: None,
                close_result: None,
            },
            owner_epoch,
            takeover_instances,
            takeover,
            journal,
        })
    }

    pub(crate) fn set_active_instances(
        &mut self,
        active_instances: impl IntoIterator<Item = InstanceId>,
    ) -> RuntimeHostResult<()> {
        let mut active_instances = active_instances.into_iter().collect::<Vec<_>>();
        active_instances.sort_unstable();
        active_instances.dedup();
        if self.record.active_instances == active_instances {
            return Ok(());
        }
        let mut record = self.record.clone();
        record.revision = record.revision.checked_add(1).ok_or_else(|| {
            RuntimeHostError::fatal(
                "owner_revision_overflow",
                "update_owner_file",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        record.active_instances = active_instances;
        append_record(self.file_mut("update_owner_file")?, &record)?;
        self.record = record;
        Ok(())
    }

    pub(crate) fn set_resource_disposition(
        &mut self,
        disposition: OwnerResourceDisposition,
    ) -> RuntimeHostResult<()> {
        if self.retained_unconfirmed {
            return Err(RuntimeHostError::fatal(
                "owner_resource_unconfirmed",
                "update_owner_resource_disposition",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        if self.record.resource_disposition == Some(disposition) {
            return Ok(());
        }
        let mut record = self.record.clone();
        record.revision = record.revision.checked_add(1).ok_or_else(|| {
            RuntimeHostError::fatal(
                "owner_revision_overflow",
                "update_owner_resource_disposition",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        record.resource_disposition = Some(disposition);
        append_record(self.file_mut("update_owner_resource_disposition")?, &record)?;
        self.record = record;
        Ok(())
    }

    pub(crate) fn retain_unconfirmed(&mut self) -> RuntimeHostResult<()> {
        if let Some(result) = &self.retention_result {
            return result.clone();
        }
        let file = self.file.take().ok_or_else(|| {
            RuntimeHostError::fatal(
                "owner_file_missing",
                "retain_unconfirmed_owner_file",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        let file = Box::leak(Box::new(file));
        self.retained_unconfirmed = true;
        let result = (|| {
            let mut record = self.record.clone();
            record.revision = record.revision.checked_add(1).ok_or_else(|| {
                RuntimeHostError::fatal(
                    "owner_revision_overflow",
                    "update_owner_resource_disposition",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
            record.resource_disposition = Some(OwnerResourceDisposition::Unconfirmed);
            append_record(file, &record)?;
            self.record = record;
            Ok(())
        })();
        self.retained_file = Some(file);
        self.retention_result = Some(result.clone());
        result
    }

    pub(crate) fn retained_resource_disposition(
        &self,
    ) -> RuntimeHostResult<Option<OwnerResourceDisposition>> {
        self.retention_result
            .as_ref()
            .map(|result| {
                result
                    .clone()
                    .map(|()| OwnerResourceDisposition::Unconfirmed)
            })
            .transpose()
    }

    pub(crate) fn close(&mut self, closed_at_unix_ms: u64) -> RuntimeHostResult<()> {
        if let Some(result) = &self.close_result {
            return result.clone();
        }
        let result = (|| {
            if self.retained_unconfirmed {
                return Err(RuntimeHostError::fatal(
                    "owner_resource_unconfirmed",
                    "close_owner_file",
                    RuntimeErrorCode::RuntimeFatal,
                ));
            }
            let mut record = self.record.clone();
            record.revision = record.revision.checked_add(1).ok_or_else(|| {
                RuntimeHostError::fatal(
                    "owner_revision_overflow",
                    "close_owner_file",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
            record.active = false;
            record.active_instances.clear();
            record.closed_at_unix_ms = Some(closed_at_unix_ms);
            record.resource_disposition = Some(OwnerResourceDisposition::None);
            append_record(self.file_mut("close_owner_file")?, &record)?;
            self.record = record;
            self.file_mut("close_owner_file")?.unlock().map_err(|_| {
                RuntimeHostError::fatal(
                    "owner_unlock_failed",
                    "close_owner_file",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
            self.closed = true;
            Ok(())
        })();
        if result.is_err()
            && let Some(file) = self.file.take()
        {
            Box::leak(Box::new(file));
            self.retained_unconfirmed = true;
        }
        self.close_result = Some(result.clone());
        result
    }
    fn file_mut(&mut self, operation: &'static str) -> RuntimeHostResult<&mut File> {
        if let Some(Err(error)) = &self.retention_result {
            return Err(error.clone());
        }
        self.file
            .as_mut()
            .or(self.retained_file.as_deref_mut())
            .ok_or_else(|| {
                RuntimeHostError::fatal(
                    "owner_file_missing",
                    operation,
                    RuntimeErrorCode::RuntimeFatal,
                )
            })
    }
}

/// A retained owner epoch whose resources the operator confirmed released. The owner file
/// stays exclusively locked until this value is dropped.
pub(crate) struct OwnerUnlock {
    _file: File,
    pub(crate) owner_epoch: OwnerEpoch,
    pub(crate) previous_resource_disposition: OwnerResourceDisposition,
    pub(crate) revision: u64,
}

/// Offline `actingd unlock-owner`: when the last record is a v2 InUse/Unconfirmed epoch,
/// appends one record to that same epoch (same pid, started_at and active instances, still
/// active) whose `ConfirmedClosed` disposition is the operator's confirmation, so the next
/// start takes it over automatically. Every refusal returns before any write.
pub(crate) fn unlock_retained_owner(
    state_root: &Path,
    confirmed: bool,
) -> RuntimeHostResult<OwnerUnlock> {
    const OPERATION: &str = "unlock_owner_file";
    let refused = |code| RuntimeHostError::fatal(code, OPERATION, RuntimeErrorCode::InvalidRequest);
    // Not created here: a state root without an owner journal has nothing to unlock.
    let mut file = match OpenOptions::new()
        .read(true)
        .write(true)
        .open(state_root.join(OWNER_FILE_NAME))
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(refused("owner_unlock_not_required"));
        }
        Err(_) => {
            return Err(RuntimeHostError::fatal(
                "owner_file_open_failed",
                "open_owner_file",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
    };
    try_lock_owner_file(&file, "owner_unlock_daemon_active", OPERATION)?;
    let Some((mut record, previous_resource_disposition)) =
        read_last_record(&mut file)?.and_then(|record| match record.resource_disposition {
            Some(
                disposition @ (OwnerResourceDisposition::InUse
                | OwnerResourceDisposition::Unconfirmed),
            ) if record.schema_version == OWNER_SCHEMA_VERSION => Some((record, disposition)),
            _ => None,
        })
    else {
        return Err(refused("owner_unlock_not_required"));
    };
    if !confirmed {
        return Err(refused("owner_unlock_confirmation_missing"));
    }
    record.revision = record.revision.checked_add(1).ok_or_else(|| {
        RuntimeHostError::fatal(
            "owner_revision_overflow",
            OPERATION,
            RuntimeErrorCode::RuntimeFatal,
        )
    })?;
    record.resource_disposition = Some(OwnerResourceDisposition::ConfirmedClosed);
    append_record(&mut file, &record)?;
    Ok(OwnerUnlock {
        _file: file,
        owner_epoch: record.owner_epoch,
        previous_resource_disposition,
        revision: record.revision,
    })
}

impl Drop for OwnerGuard {
    fn drop(&mut self) {
        if self.closed || self.retained_unconfirmed || std::thread::panicking() {
            return;
        }
        let result = crate::time::unix_ms_now().and_then(|now| self.close(now));
        if let Err(error) = result {
            panic!("{error}");
        }
    }
}

fn read_owner_journal(file: &mut File) -> RuntimeHostResult<RuntimeOwnerJournal> {
    RuntimeOwnerJournal::read_locked(file).map_err(|error| {
        RuntimeHostError::fatal(error.code, error.operation, RuntimeErrorCode::RuntimeFatal)
    })
}

fn read_last_record(file: &mut File) -> RuntimeHostResult<Option<OwnerRecord>> {
    Ok(read_owner_journal(file)?.last().cloned())
}

fn try_lock_owner_file(
    file: &File,
    conflict: &'static str,
    operation: &'static str,
) -> RuntimeHostResult<()> {
    file.try_lock().map_err(|error| match error {
        std::fs::TryLockError::WouldBlock => {
            RuntimeHostError::fatal(conflict, operation, RuntimeErrorCode::OwnerConflict)
        }
        std::fs::TryLockError::Error(_) => RuntimeHostError::fatal(
            "owner_lock_failed",
            operation,
            RuntimeErrorCode::RuntimeFatal,
        ),
    })
}

fn append_record(file: &mut File, record: &OwnerRecord) -> RuntimeHostResult<()> {
    let mut encoded = serde_json::to_vec(record).map_err(|_| {
        RuntimeHostError::fatal(
            "owner_record_encode_failed",
            "write_owner_file",
            RuntimeErrorCode::RuntimeFatal,
        )
    })?;
    encoded.push(b'\n');
    file.seek(SeekFrom::End(0)).map_err(|_| {
        RuntimeHostError::fatal(
            "owner_seek_failed",
            "write_owner_file",
            RuntimeErrorCode::RuntimeFatal,
        )
    })?;
    file.write_all(&encoded).map_err(|_| {
        RuntimeHostError::fatal(
            "owner_write_failed",
            "write_owner_file",
            RuntimeErrorCode::RuntimeFatal,
        )
    })?;
    file.sync_data().map_err(|_| {
        RuntimeHostError::fatal(
            "owner_sync_failed",
            "write_owner_file",
            RuntimeErrorCode::RuntimeFatal,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Defect regression D10: PR298 review 5120590779, Workflow #257 C1B9 v16.
    #[test]
    fn c1b9_d10_owner_persist_failure() {
        let root = tempfile::tempdir().expect("owner root");
        let issuer = IdentifierIssuer::new().expect("issuer");
        let mut owner = OwnerGuard::acquire(root.path(), &issuer, 1)
            .expect("owner")
            .guard;
        drop(owner.file.take());
        let file = OpenOptions::new()
            .read(true)
            .open(root.path().join(OWNER_FILE_NAME))
            .expect("read-only journal");
        file.try_lock().expect("actual owner lock");
        owner.file = Some(file);
        let error = owner
            .retain_unconfirmed()
            .expect_err("journal cannot be written");
        assert_eq!(error.code(), "owner_write_failed");
        assert!(owner.retained_unconfirmed);
        assert_eq!(
            owner
                .retain_unconfirmed()
                .expect_err("cached failure")
                .code(),
            error.code()
        );
        drop(owner);
        assert_eq!(
            OwnerGuard::acquire(root.path(), &issuer, 2)
                .err()
                .expect("lock retained after persistence failure")
                .code(),
            "owner_conflict"
        );
    }

    // Task Contract: Workflow #257 / C1B9. Test class: specification criterion.
    #[test]
    fn owner_protocol_unconfirmed_blocks_automatic_takeover() {
        let root = tempfile::tempdir().expect("owner root");
        let issuer = IdentifierIssuer::new().expect("identifier issuer");
        let first = OwnerGuard::acquire(root.path(), &issuer, 1).expect("first owner");
        let mut first_guard = first.guard;
        first_guard
            .retain_unconfirmed()
            .expect("retain unconfirmed owner");

        let instance = *issuer
            .mint_instance_id()
            .expect("retained instance")
            .transport();
        first_guard
            .set_active_instances([instance])
            .expect("update same retained owner");
        let record = read_last_record(
            first_guard
                .file_mut("read_retained_owner")
                .expect("same handle"),
        )
        .expect("persisted retained owner")
        .expect("owner record");
        assert_eq!(record.active_instances, [instance]);
        assert_eq!(
            record.resource_disposition,
            Some(OwnerResourceDisposition::Unconfirmed)
        );
        assert_eq!(
            first_guard
                .retained_resource_disposition()
                .expect("retained result"),
            Some(OwnerResourceDisposition::Unconfirmed)
        );
        assert_eq!(
            first_guard
                .set_resource_disposition(OwnerResourceDisposition::ConfirmedClosed)
                .expect_err("retained resources cannot be cleared")
                .code(),
            "owner_resource_unconfirmed"
        );

        let error = OwnerGuard::acquire(root.path(), &issuer, 2)
            .err()
            .expect("takeover rejected");
        assert_eq!(error.code(), "owner_conflict");

        let stale_root = tempfile::tempdir().expect("stale owner root");
        let mut stale_file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(stale_root.path().join(OWNER_FILE_NAME))
            .expect("stale owner file");
        append_record(
            &mut stale_file,
            &OwnerRecord {
                schema_version: OWNER_SCHEMA_VERSION.to_owned(),
                revision: 1,
                owner_epoch: *issuer.mint_owner_epoch().expect("stale epoch").transport(),
                pid: process::id(),
                started_at_unix_ms: 1,
                active: true,
                active_instances: Vec::new(),
                closed_at_unix_ms: None,
                resource_disposition: Some(OwnerResourceDisposition::Unconfirmed),
            },
        )
        .expect("write stale unconfirmed owner");
        drop(stale_file);
        let stale_error = OwnerGuard::acquire(stale_root.path(), &issuer, 3)
            .err()
            .expect("stale takeover rejected");
        assert_eq!(stale_error.code(), "owner_resource_unconfirmed");
    }
}
