// SPDX-License-Identifier: AGPL-3.0-only

//! Paths, clock, file IO and error helpers of the Lab recording state.

use super::model::{LAB_RECORDING_SCHEMA, LabRecording};
use actingcommand_contract::{LabError, LabErrorClass, LabResult};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static WRITE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) const RECORDING_FILE: &str = "recording.json";

pub(crate) fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// The same file-stem rule as the `record` command files (`record-<instance>.json`).
pub(crate) fn safe_file_stem(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

pub(crate) fn old_record_path(state_dir: &Path, instance: &str) -> PathBuf {
    state_dir.join(format!("record-{}.json", safe_file_stem(instance)))
}

pub(crate) fn lab_dir(state_dir: &Path, record_id: &str) -> PathBuf {
    state_dir
        .join("record-artifacts")
        .join(safe_file_stem(record_id))
        .join("lab")
}

pub(crate) fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut text = String::with_capacity(64);
    for byte in digest {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

pub(crate) fn blocked(code: &str, message: impl Into<String>) -> LabError {
    LabError::safety_blocked(code, message, &["session_record"])
}

pub(crate) fn invalid(code: &str, message: impl Into<String>) -> LabError {
    LabError::new(LabErrorClass::UsageValidation, code, message, &[])
}

pub(crate) fn state_io(message: impl Into<String>) -> LabError {
    LabError::new(
        LabErrorClass::RuntimeUnavailable,
        "record_state_io_failed",
        message,
        &["session_record"],
    )
}

pub(crate) fn with_details(error: LabError, details: Value) -> LabError {
    error.with_details(details)
}

/// Writes `bytes` to a temporary sibling and renames it over `path`. A failed rename leaves
/// the temporary file in place and names it in the error.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> LabResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| state_io(format!("{} has no parent directory", path.display())))?;
    fs::create_dir_all(parent)
        .map_err(|error| state_io(format!("failed to create {}: {error}", parent.display())))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| state_io(format!("{} has no UTF-8 file name", path.display())))?;
    let sequence = WRITE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        "{file_name}.tmp-{}-{}-{sequence}",
        std::process::id(),
        now_unix_ms()
    ));
    let mut file = File::create(&temporary)
        .map_err(|error| state_io(format!("failed to create {}: {error}", temporary.display())))?;
    file.write_all(bytes)
        .map_err(|error| state_io(format!("failed to write {}: {error}", temporary.display())))?;
    file.sync_all()
        .map_err(|error| state_io(format!("failed to sync {}: {error}", temporary.display())))?;
    drop(file);
    fs::rename(&temporary, path).map_err(|error| {
        state_io(format!(
            "failed to publish {} from {} (left in place): {error}",
            path.display(),
            temporary.display()
        ))
    })
}

pub(crate) fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> LabResult<()> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| state_io(format!("failed to encode {}: {error}", path.display())))?;
    bytes.push(b'\n');
    write_atomic(path, &bytes)
}

/// Writes content-addressed bytes once: an existing file must already hold them.
pub(crate) fn write_content_addressed(path: &Path, bytes: &[u8], sha256: &str) -> LabResult<bool> {
    match fs::read(path) {
        Ok(existing) => {
            if hex_sha256(&existing) != sha256 {
                return Err(with_details(
                    blocked(
                        "record_frame_hash_mismatch",
                        format!(
                            "{} exists but does not hold sha256 {sha256}",
                            path.display()
                        ),
                    ),
                    json!({"path": path.display().to_string(), "expected_sha256": sha256}),
                ));
            }
            Ok(false)
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            write_atomic(path, bytes)?;
            Ok(true)
        }
        Err(error) => Err(state_io(format!(
            "failed to read {}: {error}",
            path.display()
        ))),
    }
}

/// Reads a stored PNG and checks its sha256 (`record_frame_hash_mismatch`).
pub(crate) fn read_verified(path: &Path, sha256: &str) -> LabResult<Vec<u8>> {
    let bytes = fs::read(path).map_err(|error| {
        with_details(
            blocked(
                "record_frame_hash_mismatch",
                format!("failed to read stored {}: {error}", path.display()),
            ),
            json!({"path": path.display().to_string(), "expected_sha256": sha256}),
        )
    })?;
    let actual = hex_sha256(&bytes);
    if actual != sha256 {
        return Err(with_details(
            blocked(
                "record_frame_hash_mismatch",
                format!(
                    "stored {} has sha256 {actual}, recorded {sha256}",
                    path.display()
                ),
            ),
            json!({
                "path": path.display().to_string(),
                "expected_sha256": sha256,
                "actual_sha256": actual
            }),
        ));
    }
    Ok(bytes)
}

/// The fields of the `record` command file (`record-<instance>.json`) the Lab recording
/// reads; the file itself is only ever written by the `record` command.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct OldRecordView {
    pub(crate) record_id: String,
    pub(crate) task_id: String,
    pub(crate) instance: String,
    pub(crate) status: String,
    pub(crate) started_at_unix_ms: u64,
}

pub(crate) fn read_old_record(
    state_dir: &Path,
    instance: &str,
) -> LabResult<Option<OldRecordView>> {
    let path = old_record_path(state_dir, instance);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(state_io(format!(
                "failed to read {}: {error}",
                path.display()
            )));
        }
    };
    serde_json::from_str(&text).map(Some).map_err(|error| {
        blocked(
            "record_session_not_active",
            format!("failed to parse {}: {error}", path.display()),
        )
    })
}

pub(crate) enum LabFile {
    Missing,
    Unavailable(&'static str),
    Ready(Box<LabRecording>),
}

pub(crate) fn recording_path(state_dir: &Path, record_id: &str) -> PathBuf {
    lab_dir(state_dir, record_id).join(RECORDING_FILE)
}

/// Loads the Lab recording of `old` and checks the four identity fields against it.
pub(crate) fn load_lab(state_dir: &Path, old: &OldRecordView) -> LabResult<LabFile> {
    let path = recording_path(state_dir, &old.record_id);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(LabFile::Missing),
        Err(error) => {
            return Err(state_io(format!(
                "failed to read {}: {error}",
                path.display()
            )));
        }
    };
    let recording: LabRecording = serde_json::from_str(&text).map_err(|error| {
        with_details(
            blocked(
                "record_lab_unavailable",
                format!("failed to parse {}: {error}", path.display()),
            ),
            json!({"reason": "lab_recording_unreadable", "path": path.display().to_string()}),
        )
    })?;
    if recording.schema_version != LAB_RECORDING_SCHEMA
        || recording.record_id != old.record_id
        || recording.record_started_at_unix_ms != old.started_at_unix_ms
        || recording.task_id != old.task_id
        || recording.instance != old.instance
    {
        return Ok(LabFile::Unavailable("lab_recording_mismatch"));
    }
    Ok(LabFile::Ready(Box::new(recording)))
}

/// Creates `<state>/record-artifacts/<record_id>/lab/recording.json` once.
pub(crate) fn create_recording(state_dir: &Path, recording: &LabRecording) -> LabResult<PathBuf> {
    let path = recording_path(state_dir, &recording.record_id);
    let parent = path
        .parent()
        .ok_or_else(|| state_io(format!("{} has no parent directory", path.display())))?;
    fs::create_dir_all(parent)
        .map_err(|error| state_io(format!("failed to create {}: {error}", parent.display())))?;
    let mut bytes = serde_json::to_vec_pretty(recording)
        .map_err(|error| state_io(format!("failed to encode {}: {error}", path.display())))?;
    bytes.push(b'\n');
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| state_io(format!("failed to create {}: {error}", path.display())))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| state_io(format!("failed to write {}: {error}", path.display())))?;
    Ok(path)
}

pub(crate) fn save_recording(state_dir: &Path, recording: &LabRecording) -> LabResult<PathBuf> {
    let path = recording_path(state_dir, &recording.record_id);
    write_json_atomic(&path, recording)?;
    Ok(path)
}

/// `^[a-z0-9][a-z0-9_]{0,63}$`: the task id becomes the content-directory task folder.
pub(crate) fn task_id_valid_for_lab(task_id: &str) -> bool {
    let bytes = task_id.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_')
}

/// `^[a-z0-9][a-z0-9_]{0,47}$`: the page name becomes part of a page id.
pub(crate) fn page_name_valid(page: &str) -> bool {
    let bytes = page.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 48
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_')
}
