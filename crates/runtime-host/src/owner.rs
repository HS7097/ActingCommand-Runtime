// SPDX-License-Identifier: AGPL-3.0-only

use crate::codes::{HostCode, HostLocation};
use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::codes::{ContractLocation, SharedCode};
use actingcommand_contract::outcome::{Code, Location};
use actingcommand_contract::{
    IdentifierIssuer, InstanceId, OWNER_JOURNAL_LIMIT, OwnerEpoch, OwnerResourceDisposition,
    RuntimeErrorCode,
};
use actingcommand_host_metrics::ProcessProbe;
use actingcommand_ledger::owner_journal::{
    OwnerJournalError, RuntimeOwnerJournal, RuntimeOwnerRecord as OwnerRecord,
};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process;

pub(crate) const OWNER_FILE_NAME: &str = "owner.lock";
/// Workflow #191 O: the side file of an in-place compaction (`contracts/owner-journal.md`).
pub(crate) const OWNER_COMPACTION_FILE_NAME: &str = "owner.lock.compact";
/// Workflow #191 O: an in-epoch non-terminal append that would take the physical bytes
/// after the checkpoint past this folds every record but the last first.
pub(crate) const OWNER_JOURNAL_COMPACT_TAIL_BYTES: u64 = 64 * 1024;
/// The side file is the image plus one seal line.
const OWNER_COMPACTION_SEAL_ALLOWANCE: u64 = 4 * 1024;
const OWNER_SCHEMA_VERSION: &str = actingcommand_contract::OWNER_JOURNAL_SCHEMA;
/// A running process created more than this after the recorded `started_at_unix_ms` is
/// another incarnation of a reused pid. One created earlier is the recorded owner itself,
/// which writes its start only after loading its configuration and resources.
const PID_REUSE_TOLERANCE_MS: u64 = 2_000;

pub(crate) struct OwnerStartup {
    pub(crate) guard: OwnerGuard,
    pub(crate) owner_epoch: OwnerEpoch,
    pub(crate) takeover_instances: Vec<InstanceId>,
    pub(crate) takeover: bool,
    pub(crate) journal: RuntimeOwnerJournal,
    pub(crate) released_by_exit: Option<PriorOwnerReleasedByExit>,
}

/// Slice #315-B2c-2: the previous owner whose last journal record still declared resources
/// `in_use` or `unconfirmed` and whose process had exited, so startup treated them as
/// released for owner purposes, as `actingd unlock-owner --confirm-resources-released` does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PriorOwnerReleasedByExit {
    pub pid: u32,
    pub started_at_unix_ms: u64,
    pub last_disposition: OwnerResourceDisposition,
}

impl PriorOwnerReleasedByExit {
    pub(crate) fn phase(self) -> actingcommand_contract::RuntimeLifecyclePhase {
        actingcommand_contract::RuntimeLifecyclePhase::PriorEpochOwnerReleasedByExit {
            pid: self.pid,
            started_at_unix_ms: self.started_at_unix_ms,
            last_disposition: self.last_disposition,
        }
    }
}

/// The daemon's startup line for the same decision.
impl std::fmt::Display for PriorOwnerReleasedByExit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let last_disposition = match self.last_disposition {
            OwnerResourceDisposition::None => "none",
            OwnerResourceDisposition::InUse => "in_use",
            OwnerResourceDisposition::ConfirmedClosed => "confirmed_closed",
            OwnerResourceDisposition::Unconfirmed => "unconfirmed",
        };
        write!(
            formatter,
            "owner_resources_released_by_exit pid={} started_at_unix_ms={} last_disposition={last_disposition}",
            self.pid, self.started_at_unix_ms
        )
    }
}

/// A v2 InUse/Unconfirmed last record (a failed close included) is released only when its
/// process is dead: no running process has the pid, or the running one was created more
/// than `PID_REUSE_TOLERANCE_MS` after the recorded start (a reused pid cannot have been
/// created before it). Alive, or a probe that cannot decide, keeps the
/// `owner_resource_unconfirmed` refusal with the probe outcome.
fn release_exited_owner(
    previous: Option<&OwnerRecord>,
) -> RuntimeHostResult<Option<PriorOwnerReleasedByExit>> {
    let Some((record, last_disposition)) =
        previous.and_then(|record| match record.resource_disposition {
            Some(
                disposition @ (OwnerResourceDisposition::InUse
                | OwnerResourceDisposition::Unconfirmed),
            ) if record.schema_version == OWNER_SCHEMA_VERSION => Some((record, disposition)),
            _ => None,
        })
    else {
        return Ok(None);
    };
    let probe = match actingcommand_host_metrics::probe_process(record.pid) {
        ProcessProbe::Running { created_at_unix_ms }
            if created_at_unix_ms
                <= record
                    .started_at_unix_ms
                    .saturating_add(PID_REUSE_TOLERANCE_MS) =>
        {
            format!("pid {} alive", record.pid)
        }
        ProcessProbe::Unknown(reason) => format!("probe unknown: {reason}"),
        ProcessProbe::NotRunning | ProcessProbe::Running { .. } => {
            return Ok(Some(PriorOwnerReleasedByExit {
                pid: record.pid,
                started_at_unix_ms: record.started_at_unix_ms,
                last_disposition,
            }));
        }
    };
    let mut refused = RuntimeHostError::fatal(
        HostCode::OwnerResourceUnconfirmed,
        HostLocation::AcquireOwnerFile,
        RuntimeErrorCode::OwnerConflict,
    );
    refused.lifecycle.owner_probe = Some(probe);
    Err(refused)
}

pub(crate) struct OwnerGuard {
    file: Option<File>,
    retained_file: Option<&'static mut File>,
    record: OwnerRecord,
    closed: bool,
    retained_unconfirmed: bool,
    retention_result: Option<RuntimeHostResult<()>>,
    close_result: Option<RuntimeHostResult<()>>,
    side_path: PathBuf,
    /// The physical bytes after the checkpoint line.
    uncompacted_bytes: u64,
    /// A failed in-place rewrite; every later write returns it before writing a byte.
    journal_failure: Option<RuntimeHostError>,
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
                    HostCode::OwnerFileOpenFailed,
                    HostLocation::OpenOwnerFile,
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
        try_lock_owner_file(
            &file,
            RuntimeErrorCode::OwnerConflict,
            HostLocation::AcquireOwnerFile,
        )?;
        recover_compaction(state_root, &mut file)?;
        let journal = read_owner_journal(&mut file)?;
        let previous = journal.last().cloned();
        let released_by_exit = release_exited_owner(previous.as_ref())?;
        let owner_epoch = *issuer
            .mint_owner_epoch()
            .map_err(|_| {
                RuntimeHostError::fatal(
                    HostCode::OwnerEpochIssueFailed,
                    HostLocation::AcquireOwnerFile,
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
                    HostCode::OwnerRevisionOverflow,
                    HostLocation::AcquireOwnerFile,
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
            closed_by: None,
        };
        let encoded = encode_record(&record)?;
        write_record(&mut file, &encoded)?;
        Ok(OwnerStartup {
            guard: OwnerGuard {
                file: Some(file),
                retained_file: None,
                record,
                closed: false,
                retained_unconfirmed: false,
                retention_result: None,
                close_result: None,
                side_path: state_root.join(OWNER_COMPACTION_FILE_NAME),
                uncompacted_bytes: journal.uncompacted_bytes() + encoded.len() as u64,
                journal_failure: None,
            },
            owner_epoch,
            takeover_instances,
            takeover,
            journal,
            released_by_exit,
        })
    }

    pub(crate) fn set_active_instances(
        &mut self,
        active_instances: impl IntoIterator<Item = InstanceId>,
    ) -> RuntimeHostResult<()> {
        self.journal_gate()?;
        let mut active_instances = active_instances.into_iter().collect::<Vec<_>>();
        active_instances.sort_unstable();
        active_instances.dedup();
        if self.record.active_instances == active_instances {
            return Ok(());
        }
        let mut record = self.record.clone();
        record.revision = record.revision.checked_add(1).ok_or_else(|| {
            RuntimeHostError::fatal(
                HostCode::OwnerRevisionOverflow,
                HostLocation::UpdateOwnerFile,
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        record.active_instances = active_instances;
        self.append_in_epoch(&record, HostLocation::UpdateOwnerFile)?;
        self.record = record;
        Ok(())
    }

    pub(crate) fn set_resource_disposition(
        &mut self,
        disposition: OwnerResourceDisposition,
    ) -> RuntimeHostResult<()> {
        self.journal_gate()?;
        if self.retained_unconfirmed {
            return Err(RuntimeHostError::fatal(
                HostCode::OwnerResourceUnconfirmed,
                HostLocation::UpdateOwnerResourceDisposition,
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        if self.record.resource_disposition == Some(disposition) {
            return Ok(());
        }
        let mut record = self.record.clone();
        record.revision = record.revision.checked_add(1).ok_or_else(|| {
            RuntimeHostError::fatal(
                HostCode::OwnerRevisionOverflow,
                HostLocation::UpdateOwnerResourceDisposition,
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        record.resource_disposition = Some(disposition);
        self.append_in_epoch(&record, HostLocation::UpdateOwnerResourceDisposition)?;
        self.record = record;
        Ok(())
    }

    pub(crate) fn retain_unconfirmed(&mut self) -> RuntimeHostResult<()> {
        if let Some(result) = &self.retention_result {
            return result.clone();
        }
        let file = self.file.take().ok_or_else(|| {
            RuntimeHostError::fatal(
                HostCode::OwnerFileMissing,
                HostLocation::RetainUnconfirmedOwnerFile,
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        let file = Box::leak(Box::new(file));
        self.retained_unconfirmed = true;
        let result = (|| {
            self.journal_gate()?;
            let mut record = self.record.clone();
            record.revision = record.revision.checked_add(1).ok_or_else(|| {
                RuntimeHostError::fatal(
                    HostCode::OwnerRevisionOverflow,
                    HostLocation::UpdateOwnerResourceDisposition,
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
            self.journal_gate()?;
            if self.retained_unconfirmed {
                return Err(RuntimeHostError::fatal(
                    HostCode::OwnerResourceUnconfirmed,
                    HostLocation::CloseOwnerFile,
                    RuntimeErrorCode::RuntimeFatal,
                ));
            }
            let mut record = self.record.clone();
            record.revision = record.revision.checked_add(1).ok_or_else(|| {
                RuntimeHostError::fatal(
                    HostCode::OwnerRevisionOverflow,
                    HostLocation::CloseOwnerFile,
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
            record.active = false;
            record.active_instances.clear();
            record.closed_at_unix_ms = Some(closed_at_unix_ms);
            record.resource_disposition = Some(OwnerResourceDisposition::None);
            append_record(self.file_mut(HostLocation::CloseOwnerFile)?, &record)?;
            self.record = record;
            self.file_mut(HostLocation::CloseOwnerFile)?
                .unlock()
                .map_err(|_| {
                    RuntimeHostError::fatal(
                        HostCode::OwnerUnlockFailed,
                        HostLocation::CloseOwnerFile,
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

    /// Workflow #191 O: after a failed in-place rewrite no write of this guard reaches the
    /// journal; the next start finishes the compaction from its side file.
    fn journal_gate(&self) -> RuntimeHostResult<()> {
        self.journal_failure.clone().map_or(Ok(()), Err)
    }

    /// Workflow #191 O: an in-epoch non-terminal append. Past the uncompacted threshold it
    /// first folds every record but the last into the leading checkpoint; a failed fold
    /// fails the append, which is then not written.
    fn append_in_epoch(
        &mut self,
        record: &OwnerRecord,
        operation: impl Into<Location>,
    ) -> RuntimeHostResult<()> {
        let encoded = encode_record(record)?;
        if !self.retained_unconfirmed
            && self.uncompacted_bytes + encoded.len() as u64 > OWNER_JOURNAL_COMPACT_TAIL_BYTES
        {
            self.compact()?;
        }
        write_record(self.file_mut(operation)?, &encoded)?;
        self.uncompacted_bytes += encoded.len() as u64;
        Ok(())
    }

    /// S1 checks the image without writing; S2 makes the side file durable; S3 rewrites
    /// the journal in place; S4 removes the side file. Only an S3 failure is cached: the
    /// journal may then be half written, and the next start completes it (R2-R4).
    fn compact(&mut self) -> RuntimeHostResult<()> {
        const OPERATION: ContractLocation = ContractLocation::CompactOwnerFile;
        let fatal =
            |code: Code| RuntimeHostError::fatal(code, OPERATION, RuntimeErrorCode::RuntimeFatal);
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| fatal(HostCode::OwnerFileMissing.code()))?;
        let Some(compaction) =
            RuntimeOwnerJournal::compaction_locked(file).map_err(owner_journal_error)?
        else {
            return Ok(());
        };
        if stage_side_file(&self.side_path, compaction.side_file()).is_err() {
            let error = fatal(HostCode::OwnerCompactionStageFailed.code());
            return Err(match std::fs::remove_file(&self.side_path) {
                Err(cleanup) if cleanup.kind() != std::io::ErrorKind::NotFound => error
                    .with_related_failure(
                        "side_file_cleanup",
                        &fatal(HostCode::OwnerCompactionCleanupFailed.code()),
                    ),
                _ => error,
            });
        }
        let rewritten = file
            .seek(SeekFrom::Start(0))
            .and_then(|_| file.write_all(compaction.image()))
            .and_then(|()| file.set_len(compaction.image().len() as u64))
            .and_then(|()| file.sync_all());
        if rewritten.is_err() {
            let error = fatal(HostCode::OwnerCompactionRewriteFailed.code());
            self.journal_failure = Some(error.clone());
            return Err(error);
        }
        self.uncompacted_bytes = compaction.kept_bytes();
        std::fs::remove_file(&self.side_path)
            .and_then(|()| sync_directory(side_directory(&self.side_path)))
            .map_err(|_| fatal(HostCode::OwnerCompactionCleanupFailed.code()))
    }

    fn file_mut(&mut self, operation: impl Into<Location>) -> RuntimeHostResult<&mut File> {
        let operation: Location = operation.into();
        if let Some(Err(error)) = &self.retention_result {
            return Err(error.clone());
        }
        self.file
            .as_mut()
            .or(self.retained_file.as_deref_mut())
            .ok_or_else(|| {
                RuntimeHostError::fatal(
                    HostCode::OwnerFileMissing,
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
    const OPERATION: HostLocation = HostLocation::UnlockOwnerFile;
    let refused =
        |code: Code| RuntimeHostError::fatal(code, OPERATION, RuntimeErrorCode::InvalidRequest);
    // Not created here: a state root without an owner journal has nothing to unlock.
    let mut file = match OpenOptions::new()
        .read(true)
        .write(true)
        .open(state_root.join(OWNER_FILE_NAME))
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(refused(HostCode::OwnerUnlockNotRequired.code()));
        }
        Err(_) => {
            return Err(RuntimeHostError::fatal(
                HostCode::OwnerFileOpenFailed,
                HostLocation::OpenOwnerFile,
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
    };
    try_lock_owner_file(&file, HostCode::OwnerUnlockDaemonActive, OPERATION)?;
    recover_compaction(state_root, &mut file)?;
    let Some((mut record, previous_resource_disposition)) =
        read_last_record(&mut file)?.and_then(|record| match record.resource_disposition {
            Some(
                disposition @ (OwnerResourceDisposition::InUse
                | OwnerResourceDisposition::Unconfirmed),
            ) if record.schema_version == OWNER_SCHEMA_VERSION => Some((record, disposition)),
            _ => None,
        })
    else {
        return Err(refused(HostCode::OwnerUnlockNotRequired.code()));
    };
    if !confirmed {
        return Err(refused(HostCode::OwnerUnlockConfirmationMissing.code()));
    }
    record.revision = record.revision.checked_add(1).ok_or_else(|| {
        RuntimeHostError::fatal(
            HostCode::OwnerRevisionOverflow,
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

fn owner_journal_error(error: OwnerJournalError) -> RuntimeHostError {
    RuntimeHostError::fatal(error.code, error.operation, RuntimeErrorCode::RuntimeFatal)
}

fn read_owner_journal(file: &mut File) -> RuntimeHostResult<RuntimeOwnerJournal> {
    RuntimeOwnerJournal::read_locked(file).map_err(owner_journal_error)
}

/// Workflow #191 O: under the owner lock and before any read, finishes or discards a
/// compaction a crash interrupted, then removes its side file. Like the incomplete-tail
/// recovery it writes no event.
fn recover_compaction(state_root: &Path, file: &mut File) -> RuntimeHostResult<()> {
    const OPERATION: ContractLocation = ContractLocation::RecoverOwnerCompaction;
    let fatal =
        |code: Code| RuntimeHostError::fatal(code, OPERATION, RuntimeErrorCode::RuntimeFatal);
    let side_path = state_root.join(OWNER_COMPACTION_FILE_NAME);
    let mut side = Vec::new();
    match File::open(&side_path).and_then(|side_file| {
        side_file
            .take(OWNER_JOURNAL_LIMIT + OWNER_COMPACTION_SEAL_ALLOWANCE + 1)
            .read_to_end(&mut side)
    }) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(fatal(HostCode::OwnerCompactionSideReadFailed.code())),
    }
    if side.len() as u64 > OWNER_JOURNAL_LIMIT + OWNER_COMPACTION_SEAL_ALLOWANCE {
        return Err(fatal(SharedCode::OwnerCompactionUnrecoverable.code()));
    }
    RuntimeOwnerJournal::recover_compaction_locked(file, &side).map_err(owner_journal_error)?;
    std::fs::remove_file(&side_path)
        .and_then(|()| sync_directory(side_directory(&side_path)))
        .map_err(|_| fatal(HostCode::OwnerCompactionCleanupFailed.code()))
}

fn side_directory(side_path: &Path) -> &Path {
    side_path.parent().unwrap_or(Path::new("."))
}

/// S2: create or truncate, write and synchronize the side file, then its directory.
fn stage_side_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut side = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)?;
    side.write_all(bytes)?;
    side.sync_all()?;
    sync_directory(side_directory(path))
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    OpenOptions::new()
        .read(true)
        .open(path)
        .and_then(|directory| directory.sync_all())
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> std::io::Result<()> {
    // Rust's standard library cannot open Windows directories for fsync without unsafe flags.
    Ok(())
}

fn read_last_record(file: &mut File) -> RuntimeHostResult<Option<OwnerRecord>> {
    Ok(read_owner_journal(file)?.last().cloned())
}

fn try_lock_owner_file(
    file: &File,
    conflict: impl Into<Code>,
    operation: impl Into<Location>,
) -> RuntimeHostResult<()> {
    let conflict: Code = conflict.into();
    let operation: Location = operation.into();
    file.try_lock().map_err(|error| match error {
        std::fs::TryLockError::WouldBlock => {
            RuntimeHostError::fatal(conflict, operation, RuntimeErrorCode::OwnerConflict)
        }
        std::fs::TryLockError::Error(_) => RuntimeHostError::fatal(
            HostCode::OwnerLockFailed,
            operation,
            RuntimeErrorCode::RuntimeFatal,
        ),
    })
}

fn append_record(file: &mut File, record: &OwnerRecord) -> RuntimeHostResult<()> {
    write_record(file, &encode_record(record)?)
}

fn encode_record(record: &OwnerRecord) -> RuntimeHostResult<Vec<u8>> {
    let mut encoded = serde_json::to_vec(record).map_err(|_| {
        RuntimeHostError::fatal(
            HostCode::OwnerRecordEncodeFailed,
            HostLocation::WriteOwnerFile,
            RuntimeErrorCode::RuntimeFatal,
        )
    })?;
    encoded.push(b'\n');
    Ok(encoded)
}

fn write_record(file: &mut File, encoded: &[u8]) -> RuntimeHostResult<()> {
    file.seek(SeekFrom::End(0)).map_err(|_| {
        RuntimeHostError::fatal(
            SharedCode::OwnerSeekFailed,
            HostLocation::WriteOwnerFile,
            RuntimeErrorCode::RuntimeFatal,
        )
    })?;
    file.write_all(encoded).map_err(|_| {
        RuntimeHostError::fatal(
            HostCode::OwnerWriteFailed,
            HostLocation::WriteOwnerFile,
            RuntimeErrorCode::RuntimeFatal,
        )
    })?;
    file.sync_data().map_err(|_| {
        RuntimeHostError::fatal(
            HostCode::OwnerSyncFailed,
            HostLocation::WriteOwnerFile,
            RuntimeErrorCode::RuntimeFatal,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_ledger::owner_journal::OwnerCloseMarker;

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
                .file_mut(crate::codes::test_spellings::TestLocation::ReadRetainedOwner)
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

        // Slice #315-B2c-2: an unlocked Unconfirmed record whose pid is a running process.
        let stale_root = |started_at_unix_ms| {
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
                    started_at_unix_ms,
                    active: true,
                    active_instances: Vec::new(),
                    closed_at_unix_ms: None,
                    resource_disposition: Some(OwnerResourceDisposition::Unconfirmed),
                    closed_by: None,
                },
            )
            .expect("write stale unconfirmed owner");
            stale_root
        };
        // The recorded start matches this process's creation time: still alive, refused.
        let ProcessProbe::Running { created_at_unix_ms } =
            actingcommand_host_metrics::probe_process(process::id())
        else {
            panic!("own process probe must report a running process");
        };
        let alive_root = stale_root(created_at_unix_ms);
        let stale_error = OwnerGuard::acquire(alive_root.path(), &issuer, 3)
            .err()
            .expect("stale takeover rejected");
        assert_eq!(stale_error.code(), "owner_resource_unconfirmed");
        assert_eq!(
            stale_error.lifecycle.owner_probe,
            Some(format!("pid {} alive", process::id()))
        );

        // A creation time far from the recorded start is a reused pid: the previous owner is
        // dead and its epoch is taken over without an unlock record.
        let reused_root = stale_root(1);
        let OwnerStartup {
            guard: mut released,
            owner_epoch,
            takeover,
            released_by_exit,
            ..
        } = OwnerGuard::acquire(reused_root.path(), &issuer, 4).expect("dead owner released");
        assert!(takeover);
        assert_eq!(
            released_by_exit,
            Some(PriorOwnerReleasedByExit {
                pid: process::id(),
                started_at_unix_ms: 1,
                last_disposition: OwnerResourceDisposition::Unconfirmed,
            })
        );
        let record = read_last_record(
            released
                .file_mut(crate::codes::test_spellings::TestLocation::ReadReleasedOwner)
                .expect("handle"),
        )
        .expect("persisted released owner")
        .expect("owner record");
        assert_eq!(record.revision, 2);
        assert_eq!(record.owner_epoch, owner_epoch);
        assert!(record.active);
        assert_eq!(
            record.resource_disposition,
            Some(OwnerResourceDisposition::None)
        );
        released.close(5).expect("close released owner");
    }

    /// Workflow #389 TU-44 (unified interface model v2.2.3, section 8.2; section 3.6): this build
    /// reads an owner journal whose close carries the upgrade-state marker as usual, with the
    /// marker on an ordinary record and on a checkpoint's `last_record`, and the owner record
    /// schema still v2; a journal without the marker reads as before. actingd start and
    /// `ledger-maintenance` read the journal through `OwnerGuard::acquire`.
    #[test]
    fn tu44_an_owner_journal_with_the_upgrade_state_close_marker_reads_as_usual() {
        const OWNER_SCHEMA_V2: &str = "actingcommand.runtime-owner.v2";
        let root = tempfile::tempdir().expect("owner root");
        let path = root.path().join(OWNER_FILE_NAME);
        let issuer = IdentifierIssuer::new().expect("identifier issuer");
        let startup_shape = |startup: &OwnerStartup| {
            (
                startup.takeover,
                startup.takeover_instances.clone(),
                startup.released_by_exit,
            )
        };
        let own_revision = |guard: &mut OwnerGuard| {
            read_last_record(
                guard
                    .file_mut(crate::codes::test_spellings::TestLocation::ReadOwnStart)
                    .expect("same handle"),
            )
            .expect("persisted start")
            .expect("owner record")
            .revision
        };

        // Without the marker: a journal ending in a formal close reads as before.
        let mut first = OwnerGuard::acquire(root.path(), &issuer, 10)
            .expect("first owner")
            .guard;
        first.close(11).expect("first close");
        drop(first);
        let mut second = OwnerGuard::acquire(root.path(), &issuer, 12).expect("unmarked journal");
        let previous = second.journal.last().expect("first close record");
        assert_eq!(
            (
                previous.revision,
                previous.active,
                previous.closed_at_unix_ms,
                previous.resource_disposition,
                previous.closed_by,
            ),
            (
                2,
                false,
                Some(11),
                Some(OwnerResourceDisposition::None),
                None
            )
        );
        assert_eq!(startup_shape(&second), (false, Vec::new(), None));
        assert_eq!(own_revision(&mut second.guard), 3);
        second.guard.close(13).expect("second close");
        let unmarked_close = second.guard.record.clone();
        drop(second);

        // The upgrade state's close (written from #389 U-4 on): the same record with the marker.
        let journal = std::fs::read_to_string(&path).expect("owner journal");
        let body = journal.strip_suffix("}\n").expect("complete close record");
        std::fs::write(&path, format!("{body},\"closed_by\":\"upgrade_state\"}}\n"))
            .expect("marked close");

        // The marker on an ordinary record.
        let mut third = OwnerGuard::acquire(root.path(), &issuer, 14).expect("marked journal");
        let marked = third.journal.last().expect("marked close record");
        assert_eq!(marked.schema_version, OWNER_SCHEMA_V2);
        assert_eq!(marked.closed_by, Some(OwnerCloseMarker::UpgradeState));
        assert_eq!(
            serde_json::to_value(OwnerRecord {
                closed_by: None,
                ..marked.clone()
            })
            .expect("encode marked close"),
            serde_json::to_value(unmarked_close).expect("encode unmarked close")
        );
        assert_eq!(startup_shape(&third), (false, Vec::new(), None));
        assert_eq!(own_revision(&mut third.guard), 5);

        // The marker on a checkpoint's `last_record`: this owner folds every record but its own
        // start, as an in-epoch append past the compaction threshold does.
        third
            .guard
            .compact()
            .expect("fold through the marked close");
        third.guard.close(15).expect("third close");
        drop(third);
        let journal = std::fs::read_to_string(&path).expect("folded journal");
        let checkpoint: serde_json::Value =
            serde_json::from_str(journal.lines().next().expect("checkpoint line"))
                .expect("checkpoint");
        assert_eq!(
            checkpoint["schema_version"],
            "actingcommand.runtime-owner-checkpoint.v1"
        );
        assert_eq!(checkpoint["last_record"]["schema_version"], OWNER_SCHEMA_V2);
        assert_eq!(checkpoint["last_record"]["revision"], 4);
        assert_eq!(checkpoint["last_record"]["closed_by"], "upgrade_state");
        let mut fourth =
            OwnerGuard::acquire(root.path(), &issuer, 16).expect("folded marked journal");
        let previous = fourth.journal.last().expect("third close record");
        assert_eq!((previous.revision, previous.active), (6, false));
        assert_eq!(startup_shape(&fourth), (false, Vec::new(), None));
        assert_eq!(own_revision(&mut fourth.guard), 7);
        fourth.guard.close(17).expect("fourth close");
    }
}
