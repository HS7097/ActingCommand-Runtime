// SPDX-License-Identifier: AGPL-3.0-only

//! R20: one operating-system file lock per instance and state root, taken by every
//! state-writing recording command. The lock is released with the process handle, so a
//! crashed or killed holder leaves no stale lock behind.

use super::store::{now_unix_ms, safe_file_stem, write_json_atomic};
use actingcommand_contract::{LabError, LabErrorClass, LabResult};
use serde::Serialize;
use serde_json::{Value, json};
use std::fs::{self, File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize)]
struct LockHolder<'a> {
    pid: u32,
    command: &'a str,
    acquired_at_unix_ms: u64,
}

/// Held for one command process; dropping it closes the handle and releases the lock.
#[derive(Debug)]
pub struct RecordingLock {
    _file: File,
    state_dir: PathBuf,
    instance: String,
    lock_path: PathBuf,
}

impl RecordingLock {
    /// Takes `<state>/record-<instance>.lock` without waiting and records the holder in
    /// `<state>/record-<instance>.lock.json`.
    pub fn acquire(state_dir: &Path, instance: &str, command: &str) -> LabResult<Self> {
        let stem = safe_file_stem(instance);
        let lock_path = state_dir.join(format!("record-{stem}.lock"));
        let holder_path = state_dir.join(format!("record-{stem}.lock.json"));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|error| lock_failed(instance, &lock_path, &error.to_string()))?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(busy(instance, &lock_path, read_holder(&holder_path)));
            }
            Err(TryLockError::Error(error)) => {
                return Err(lock_failed(instance, &lock_path, &error.to_string()));
            }
        }
        write_json_atomic(
            &holder_path,
            &LockHolder {
                pid: std::process::id(),
                command,
                acquired_at_unix_ms: now_unix_ms(),
            },
        )
        .map_err(|error| lock_failed(instance, &holder_path, &error.message))?;
        Ok(Self {
            _file: file,
            state_dir: state_dir.to_path_buf(),
            instance: instance.to_string(),
            lock_path,
        })
    }

    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    pub fn instance(&self) -> &str {
        &self.instance
    }

    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }
}

fn read_holder(path: &Path) -> Value {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or(Value::Null)
}

fn busy(instance: &str, lock_path: &Path, holder: Value) -> LabError {
    let pid = holder
        .get("pid")
        .map(Value::to_string)
        .unwrap_or_else(|| "unknown".to_string());
    let command = holder
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    LabError::new(
        LabErrorClass::SafetyBlocked,
        "record_busy",
        format!(
            "the recording of instance {instance} is being changed by another process \
             (pid {pid}, command {command}); wait for it to finish, run `record status` to see \
             the current step, then decide whether to redo the command; a `record mark` \
             without --step retried now may land on a step the other process just opened"
        ),
        &["record_lock"],
    )
    .with_details(json!({
        "instance": instance,
        "lock_path": lock_path.display().to_string(),
        "holder": holder
    }))
}

fn lock_failed(instance: &str, path: &Path, reason: &str) -> LabError {
    LabError::new(
        LabErrorClass::RuntimeUnavailable,
        "record_lock_failed",
        format!(
            "failed to take the recording lock {} for instance {instance}: {reason}",
            path.display()
        ),
        &["record_lock"],
    )
    .with_details(json!({
        "instance": instance,
        "lock_path": path.display().to_string()
    }))
}
