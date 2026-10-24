// SPDX-License-Identifier: AGPL-3.0-only

//! File helpers of `actingctl install` (Workflow #421 INST-1): whole-file reads, SHA-256, the atomic
//! write (`.part`, flush, rename), directory listings, and the relative paths a manifest and a
//! journal name with `/`. Nothing here deletes anything.

use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{IoOp, Outcome, RawSource};
use serde::Serialize;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// The OS failure of `op` on `path`, as its `foreign_os_error` link.
pub(crate) fn os_failure(path: &Path, error: &io::Error, op: IoOp) -> Outcome {
    Outcome::from_io_error(error, op).path(path)
}

/// A JSON parser's refusal: `foreign_data_error` with `raw_source` json and its message.
pub(crate) fn json_failure(error: &serde_json::Error) -> Outcome {
    Outcome::new(ContractCode::ForeignDataError)
        .raw_source(RawSource::Json)
        .raw_text(error.to_string())
}

/// A document of another schema than the one this build writes: `foreign_data_error` with
/// `raw_source` json, the schema it names as `raw_text`, and its `path`.
pub(crate) fn json_schema_failure(path: &Path, schema: &str) -> Outcome {
    Outcome::new(ContractCode::ForeignDataError)
        .raw_source(RawSource::Json)
        .raw_text(schema)
        .path(path)
}

/// A zip reader's refusal: `foreign_data_error` with `raw_source` zip and its message.
pub(crate) fn zip_failure(error: &zip::result::ZipError) -> Outcome {
    Outcome::new(ContractCode::ForeignDataError)
        .raw_source(RawSource::Zip)
        .raw_text(error.to_string())
}

/// Reads `path` whole.
pub(crate) fn read_all(path: &Path) -> Result<Vec<u8>, Outcome> {
    fs::read(path).map_err(|error| os_failure(path, &error, IoOp::Read))
}

/// Bytes without a leading UTF-8 byte order mark.
pub(crate) fn without_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes)
}

/// Reads and parses a JSON document.
pub(crate) fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, Outcome> {
    let bytes = read_all(path)?;
    serde_json::from_slice(without_bom(&bytes)).map_err(|error| json_failure(&error).path(path))
}

/// The SHA-256 (lowercase hex) and the size of a file, read in pieces.
pub(crate) fn sha256_file(path: &Path) -> io::Result<(String, u64)> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut size = 0_u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        size += read as u64;
        hasher.update(&buffer[..read]);
    }
    Ok((format!("{:x}", hasher.finalize()), size))
}

/// The SHA-256 (lowercase hex) of bytes.
pub(crate) fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Whether `text` is 64 lowercase hex digits.
pub(crate) fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(is_lower_hex)
}

/// Whether `text` is a full commit: 40 lowercase hex digits.
pub(crate) fn is_commit(text: &str) -> bool {
    text.len() == 40 && text.bytes().all(is_lower_hex)
}

fn is_lower_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
}

/// Writes `value` as pretty JSON to `path` atomically: `<path>.part`, flushed to disk, renamed
/// over `path`.
pub(crate) fn write_json_atomic(path: &Path, value: &impl Serialize) -> Result<(), Outcome> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|error| json_failure(&error))?;
    bytes.push(b'\n');
    write_atomic(path, &bytes)
}

/// Writes `bytes` to `path` atomically: `<path>.part`, flushed to disk, renamed over `path`.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), Outcome> {
    let part = with_suffix(path, ".part");
    {
        let mut file =
            File::create(&part).map_err(|error| os_failure(&part, &error, IoOp::Create))?;
        file.write_all(bytes)
            .map_err(|error| os_failure(&part, &error, IoOp::Write))?;
        file.sync_all()
            .map_err(|error| os_failure(&part, &error, IoOp::Sync))?;
    }
    fs::rename(&part, path).map_err(|error| os_failure(path, &error, IoOp::Rename))
}

/// Creates `path` and writes `bytes` to it only when it does not exist yet; `Ok(false)` when it
/// does (nothing written).
pub(crate) fn create_new(path: &Path, bytes: &[u8]) -> Result<bool, Outcome> {
    let mut file = match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => return Ok(false),
        Err(error) => return Err(os_failure(path, &error, IoOp::Create)),
    };
    file.write_all(bytes)
        .map_err(|error| os_failure(path, &error, IoOp::Write))?;
    file.sync_all()
        .map_err(|error| os_failure(path, &error, IoOp::Sync))?;
    Ok(true)
}

/// `path` with `suffix` appended to its file name.
pub(crate) fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path
        .file_name()
        .map_or_else(Default::default, ToOwned::to_owned);
    name.push(suffix);
    path.with_file_name(name)
}

/// Creates a directory and its parents.
pub(crate) fn create_dir_all(path: &Path) -> Result<(), Outcome> {
    fs::create_dir_all(path).map_err(|error| os_failure(path, &error, IoOp::CreateDir))
}

/// Renames `from` to `to`; a failure names `from`.
pub(crate) fn rename(from: &Path, to: &Path) -> Result<(), Outcome> {
    fs::rename(from, to).map_err(|error| os_failure(from, &error, IoOp::Rename))
}

/// Copies the file `from` to `to` (its parent created), flushed to disk.
pub(crate) fn copy_file(from: &Path, to: &Path) -> Result<(), Outcome> {
    if let Some(parent) = to.parent() {
        create_dir_all(parent)?;
    }
    let mut source = File::open(from).map_err(|error| os_failure(from, &error, IoOp::Open))?;
    let mut target = File::create(to).map_err(|error| os_failure(to, &error, IoOp::Create))?;
    io::copy(&mut source, &mut target).map_err(|error| os_failure(to, &error, IoOp::Write))?;
    target
        .sync_all()
        .map_err(|error| os_failure(to, &error, IoOp::Sync))
}

/// A path under `base` named by a relative path with `/` separators.
pub(crate) fn join_relative(base: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(base.to_path_buf(), |path, part| path.join(part))
}

/// Whether a relative path with `/` separators stays below its base: no empty, `.` or `..`
/// component, no drive or `\`.
pub(crate) fn is_plain_relative(relative: &str) -> bool {
    !relative.is_empty()
        && relative.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && !part.contains(['\\', ':'])
                && !part.chars().any(char::is_control)
        })
}

/// Every file below `base`, as relative paths with `/` separators, sorted.
pub(crate) fn list_files(base: &Path) -> Result<Vec<String>, Outcome> {
    let mut files = Vec::new();
    let mut pending = vec![(base.to_path_buf(), String::new())];
    while let Some((directory, prefix)) = pending.pop() {
        let entries = fs::read_dir(&directory)
            .map_err(|error| os_failure(&directory, &error, IoOp::ReadDir))?;
        for entry in entries {
            let entry = entry.map_err(|error| os_failure(&directory, &error, IoOp::ReadDir))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let relative = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            let kind = entry
                .file_type()
                .map_err(|error| os_failure(&entry.path(), &error, IoOp::Metadata))?;
            if kind.is_dir() {
                pending.push((entry.path(), relative));
            } else {
                files.push(relative);
            }
        }
    }
    files.sort();
    Ok(files)
}

/// The ordinary spelling of a path for output: without the `\\?\` prefix of a canonical
/// Windows path.
pub(crate) fn plain(path: &Path) -> String {
    let text = path.display().to_string();
    match text.strip_prefix(r"\\?\UNC\") {
        Some(rest) => format!(r"\\{rest}"),
        None => text
            .strip_prefix(r"\\?\")
            .map_or_else(|| text.clone(), str::to_owned),
    }
}
