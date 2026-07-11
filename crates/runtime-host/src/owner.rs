// SPDX-License-Identifier: AGPL-3.0-only

use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::{IdentifierIssuer, InstanceId, OwnerEpoch, RuntimeErrorCode};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process;

pub(crate) const OWNER_FILE_NAME: &str = "owner.lock";
const OWNER_SCHEMA_VERSION: &str = "actingcommand.runtime-owner.v1";
const MAX_OWNER_JOURNAL_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnerRecord {
    schema_version: String,
    revision: u64,
    owner_epoch: OwnerEpoch,
    pid: u32,
    started_at_unix_ms: u64,
    active: bool,
    active_instances: Vec<InstanceId>,
    closed_at_unix_ms: Option<u64>,
}

pub(crate) struct OwnerStartup {
    pub(crate) guard: OwnerGuard,
    pub(crate) owner_epoch: OwnerEpoch,
    pub(crate) takeover_instances: Vec<InstanceId>,
    pub(crate) takeover: bool,
}

pub(crate) struct OwnerGuard {
    file: File,
    record: OwnerRecord,
    closed: bool,
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
        file.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => RuntimeHostError::fatal(
                "owner_conflict",
                "acquire_owner_file",
                RuntimeErrorCode::OwnerConflict,
            ),
            std::fs::TryLockError::Error(_) => RuntimeHostError::fatal(
                "owner_lock_failed",
                "acquire_owner_file",
                RuntimeErrorCode::RuntimeFatal,
            ),
        })?;
        let previous = read_last_record(&mut file)?;
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
        };
        append_record(&mut file, &record)?;
        Ok(OwnerStartup {
            guard: OwnerGuard {
                file,
                record,
                closed: false,
            },
            owner_epoch,
            takeover_instances,
            takeover,
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
        self.record.revision = self.record.revision.checked_add(1).ok_or_else(|| {
            RuntimeHostError::fatal(
                "owner_revision_overflow",
                "update_owner_file",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        self.record.active_instances = active_instances;
        append_record(&mut self.file, &self.record)
    }

    pub(crate) fn close(&mut self, closed_at_unix_ms: u64) -> RuntimeHostResult<()> {
        if self.closed {
            return Ok(());
        }
        self.record.revision = self.record.revision.checked_add(1).ok_or_else(|| {
            RuntimeHostError::fatal(
                "owner_revision_overflow",
                "close_owner_file",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        self.record.active = false;
        self.record.active_instances.clear();
        self.record.closed_at_unix_ms = Some(closed_at_unix_ms);
        append_record(&mut self.file, &self.record)?;
        self.file.unlock().map_err(|_| {
            RuntimeHostError::fatal(
                "owner_unlock_failed",
                "close_owner_file",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        self.closed = true;
        Ok(())
    }
}

impl Drop for OwnerGuard {
    fn drop(&mut self) {
        if self.closed || std::thread::panicking() {
            return;
        }
        let result = crate::time::unix_ms_now().and_then(|now| self.close(now));
        if let Err(error) = result {
            panic!("{error}");
        }
    }
}

fn read_last_record(file: &mut File) -> RuntimeHostResult<Option<OwnerRecord>> {
    let length = file
        .metadata()
        .map_err(|_| {
            RuntimeHostError::fatal(
                "owner_metadata_failed",
                "read_owner_file",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?
        .len();
    if length > MAX_OWNER_JOURNAL_BYTES {
        return Err(RuntimeHostError::fatal(
            "owner_journal_too_large",
            "read_owner_file",
            RuntimeErrorCode::RuntimeFatal,
        ));
    }
    file.seek(SeekFrom::Start(0)).map_err(|_| {
        RuntimeHostError::fatal(
            "owner_seek_failed",
            "read_owner_file",
            RuntimeErrorCode::RuntimeFatal,
        )
    })?;
    let mut content = Vec::new();
    file.read_to_end(&mut content).map_err(|_| {
        RuntimeHostError::fatal(
            "owner_read_failed",
            "read_owner_file",
            RuntimeErrorCode::RuntimeFatal,
        )
    })?;
    if content.is_empty() {
        return Ok(None);
    }
    let complete_length = if content.last() == Some(&b'\n') {
        content.len()
    } else {
        content
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map(|index| index + 1)
            .ok_or_else(|| invalid_owner_record("recover_owner_file"))?
    };
    if complete_length < content.len() {
        file.set_len(complete_length as u64).map_err(|_| {
            RuntimeHostError::fatal(
                "owner_tail_truncate_failed",
                "recover_owner_file",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        file.sync_data().map_err(|_| {
            RuntimeHostError::fatal(
                "owner_tail_sync_failed",
                "recover_owner_file",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        content.truncate(complete_length);
    }
    let content =
        std::str::from_utf8(&content).map_err(|_| invalid_owner_record("read_owner_file"))?;
    let mut previous_revision = 0;
    let mut last = None;
    for line in content.lines().filter(|line| !line.trim().is_empty()) {
        let record = serde_json::from_str::<OwnerRecord>(line)
            .map_err(|_| invalid_owner_record("read_owner_file"))?;
        validate_record(&record)?;
        if record.revision != previous_revision + 1 {
            return Err(invalid_owner_record("validate_owner_file"));
        }
        previous_revision = record.revision;
        last = Some(record);
    }
    last.ok_or_else(|| invalid_owner_record("read_owner_file"))
        .map(Some)
}

fn validate_record(record: &OwnerRecord) -> RuntimeHostResult<()> {
    let mut sorted = record.active_instances.clone();
    sorted.sort_unstable();
    sorted.dedup();
    if record.schema_version != OWNER_SCHEMA_VERSION
        || record.revision == 0
        || record.pid == 0
        || record.started_at_unix_ms == 0
        || sorted != record.active_instances
        || (record.active && record.closed_at_unix_ms.is_some())
        || (!record.active && record.closed_at_unix_ms.is_none())
    {
        return Err(RuntimeHostError::fatal(
            "owner_record_invalid",
            "validate_owner_file",
            RuntimeErrorCode::RuntimeFatal,
        ));
    }
    Ok(())
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

fn invalid_owner_record(operation: &'static str) -> RuntimeHostError {
    RuntimeHostError::fatal(
        "owner_record_invalid",
        operation,
        RuntimeErrorCode::RuntimeFatal,
    )
}
