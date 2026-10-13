// SPDX-License-Identifier: AGPL-3.0-only

//! Read-only observation for one tick: the A/B installation, the selected configuration's
//! state root, the owner lock and journal, runtime-info, FATAL logs, acsetup's writer lock and
//! the Runtime processes under the root. Nothing here writes, and the owner journal is never
//! locked.

use super::decide::{Fatal, Journal, LiveOwner, Observed, OwnerLock, OwnerRecord, RuntimeProcess};
use super::{io_failure, json_failure, powershell};
use crate::codes::ActingctlCode;
use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{IoOp, Outcome, RawSource};
use actingcommand_contract::{
    EventActor, EventSource, InstalledProcess, OWNER_JOURNAL_LIMIT, RUNTIME_INFO_FILE,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use serde_json::Value;
use std::fs::{self, File};
use std::io::{ErrorKind, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The journal the Runtime owner guard appends to (runtime-host `OWNER_FILE_NAME`).
const OWNER_JOURNAL_FILE: &str = "owner.lock";
const OWNER_SCHEMA_V1: &str = "actingcommand.runtime-owner.v1";
const OWNER_SCHEMA_V2: &str = "actingcommand.runtime-owner.v2";
const OWNER_CHECKPOINT_SCHEMA: &str = "actingcommand.runtime-owner-checkpoint.v1";
/// acsetup holds this file exclusively for every installation or configuration write.
const WRITER_LOCK: &str = "install/writer.lock";
/// The tail of a candidate log that is searched for a FATAL line.
const LOG_TAIL_BYTES: u64 = 64 * 1024;
/// Workflow #381 A R5b: how many owners before the last one a formal close without a log looks
/// back over (an installer's chained installs leave a few maintenance owners in between).
const LOOKBACK_OWNERS: usize = 16;
const MAX_RUNTIME_INFO_BYTES: u64 = 64 * 1024;
pub(crate) const RUNTIME_IMAGE: &str = "actingcommand-actingd.exe";
const HEALTH_TIMEOUT: Duration = Duration::from_secs(5);

/// The A/B installation this tick supervises. Holding it keeps the selected slot pinned with
/// a shared slot lock for the tick, as every acforward launch does.
pub(crate) struct Installation {
    pub(crate) root: PathBuf,
    /// The ordinary spelling handed to children, cmd, WMI and the task (review L7).
    pub(crate) root_plain: PathBuf,
    pub(crate) generation: u64,
    pub(crate) slot: &'static str,
    pub(crate) selection_json: String,
    pub(crate) config_plain: PathBuf,
    pub(crate) state_root: PathBuf,
    _selection: InstalledProcess,
}

impl Installation {
    /// The selection comes from `process_installation()` (the snapshot acforward handed this
    /// process) or, for a direct run, from `install\active.json`; only A/B installs qualify.
    pub(crate) fn resolve(root_argument: &Path) -> Result<Self, Outcome> {
        let root = fs::canonicalize(root_argument).map_err(|error| {
            io_failure(
                ActingctlCode::WatchdogRootUnavailable,
                root_argument,
                &error,
                IoOp::Canonicalize,
            )
        })?;
        // The process installation, or `install\active.json` and its inputs, unreadable: the
        // installation contract's own code.
        let inherited = actingcommand_contract::process_installation()
            .map_err(|error| Outcome::new(error.code()))?;
        let selected = match inherited {
            Some(selected) if selected.root() == root => selected.clone(),
            // This actingctl belongs to another installation.
            Some(selected) => {
                return Err(Outcome::new(ActingctlCode::WatchdogRootMismatch)
                    .path(plain_path(&root))
                    .other_root(plain_path(selected.root())));
            }
            None => match InstalledProcess::read_active(&root) {
                Ok(Some(selected)) => selected,
                // No install\active.json: the watchdog supervises A/B installs only.
                Ok(None) => {
                    return Err(
                        Outcome::new(ActingctlCode::WatchdogLayoutUnknown).path(plain_path(&root))
                    );
                }
                Err(error) => return Err(Outcome::new(error.code())),
            },
        };
        Ok(Self {
            state_root: selected_state_root(&selected)?,
            root_plain: plain_path(&root),
            generation: selected.selection().generation,
            slot: selected.selection().slot.as_str(),
            selection_json: selected.selection_json().to_owned(),
            config_plain: plain_path(&selected.config_path()),
            root,
            _selection: selected,
        })
    }

    // Workflow #389 MCP-10: the fixed entry is `start::Target::entry`, shared with `actingctl start`.

    /// Review M3: `install\active.json` read again under the shared writer lock, so it cannot
    /// change before the fixed entry reads it once more.
    pub(crate) fn selection_unchanged(&self) -> Result<bool, Outcome> {
        match InstalledProcess::read_active(&self.root) {
            Ok(Some(current)) => Ok(current.selection_json() == self.selection_json),
            Ok(None) => Ok(false),
            // install\active.json is unreadable before the start.
            Err(error) => Err(Outcome::new(error.code())),
        }
    }
}

fn selected_state_root(selected: &InstalledProcess) -> Result<PathBuf, Outcome> {
    let config = plain_path(&selected.config_path());
    let bytes = selected
        .config_bytes()
        .map_err(|error| Outcome::new(error.code()))?;
    let document: Value = serde_json::from_slice(&bytes).map_err(|error| {
        Outcome::new(ActingctlCode::WatchdogConfigUnreadable)
            .path(&config)
            .caused_by(json_failure(&error))
    })?;
    // The selected config's state_root must be an absolute path.
    let state_root = document
        .get("state_root")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| Outcome::new(ActingctlCode::WatchdogStateRootInvalid).path(&config))?;
    Ok(state_root)
}

/// The canonical installation root carries the Windows verbatim prefix; cmd, WMI, the task
/// definition and child working directories take the ordinary spelling of the same path.
pub(crate) fn plain_path(path: &Path) -> PathBuf {
    let Some(text) = path.to_str() else {
        return path.to_path_buf();
    };
    if let Some(share) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{share}"))
    } else if let Some(drive) = text
        .strip_prefix(r"\\?\")
        .filter(|rest| rest.as_bytes().get(1) == Some(&b':'))
    {
        PathBuf::from(drive)
    } else {
        path.to_path_buf()
    }
}

pub(crate) fn unix_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(0)
}

/// L and J. A read refused by the owner's lock (os error 33, WouldBlock elsewhere) is Locked;
/// only complete lines count, so a torn tail of an append in progress is ignored.
pub(crate) fn owner_journal(state_root: &Path) -> Result<(OwnerLock, Journal), Outcome> {
    read_owner_journal(state_root).map(|(lock, journal, _)| (lock, journal))
}

/// L and J, with the bytes of an unlocked journal (R5b looks back in them). An unreadable
/// journal is `watchdog_journal_unreadable` (`path`); an unrecognised last record carries
/// `watchdog_journal_unrecognised` with the journal's `path`.
fn read_owner_journal(state_root: &Path) -> Result<(OwnerLock, Journal, Vec<u8>), Outcome> {
    let path = state_root.join(OWNER_JOURNAL_FILE);
    let unreadable = |error: &std::io::Error, op: IoOp| {
        io_failure(ActingctlCode::WatchdogJournalUnreadable, &path, error, op)
    };
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok((OwnerLock::Missing, Journal::Absent, Vec::new()));
        }
        Err(error) => return Err(unreadable(&error, IoOp::Open)),
    };
    let modified_unix_ms = file
        .metadata()
        .and_then(|metadata| metadata.modified())
        .map(unix_ms)
        .map_err(|error| unreadable(&error, IoOp::Metadata))?;
    let mut bytes = Vec::new();
    match file.take(OWNER_JOURNAL_LIMIT + 1).read_to_end(&mut bytes) {
        Err(error) if error.kind() == ErrorKind::WouldBlock || error.raw_os_error() == Some(33) => {
            return Ok((OwnerLock::Locked, Journal::Absent, Vec::new()));
        }
        Err(error) => return Err(unreadable(&error, IoOp::Read)),
        Ok(_) if bytes.len() as u64 > OWNER_JOURNAL_LIMIT => {
            return Err(Outcome::new(ActingctlCode::WatchdogJournalUnreadable)
                .path(&path)
                .limit_bytes(OWNER_JOURNAL_LIMIT));
        }
        Ok(_) => {}
    }
    let journal = match parse_journal(&bytes, modified_unix_ms) {
        Journal::Unrecognised { failure } => Journal::Unrecognised {
            failure: failure.path(&path),
        },
        journal => journal,
    };
    Ok((OwnerLock::Unlocked, journal, bytes))
}

pub(crate) fn parse_journal(bytes: &[u8], modified_unix_ms: u64) -> Journal {
    let complete = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |end| end + 1);
    let text = match std::str::from_utf8(&bytes[..complete]) {
        Ok(text) => text,
        Err(error) => return unrecognised(not_utf8(&error)),
    };
    let Some(line) = text.lines().rev().find(|line| !line.trim().is_empty()) else {
        return Journal::Absent;
    };
    let record = serde_json::from_str::<Value>(line)
        .map_err(|error| journal_unrecognised().caused_by(json_failure(&error)))
        .and_then(|value| owner_line(&value));
    match record {
        Ok(record) => Journal::Record {
            record,
            modified_unix_ms,
        },
        Err(failure) => unrecognised(failure),
    }
}

/// Workflow #381 A R5b: each owner's last record, oldest first: one entry per run of
/// consecutive records of one epoch, from a checkpoint's `last_record` and the physical
/// records. A complete line that is no owner record is `watchdog_journal_unrecognised`.
fn owner_blocks(bytes: &[u8]) -> Result<Vec<OwnerRecord>, Outcome> {
    let complete = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |end| end + 1);
    let text = std::str::from_utf8(&bytes[..complete]).map_err(|error| not_utf8(&error))?;
    let mut owners: Vec<OwnerRecord> = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let value: Value = serde_json::from_str(line)
            .map_err(|error| journal_unrecognised().caused_by(json_failure(&error)))?;
        let record = owner_line(&value)?;
        match owners.last_mut() {
            Some(last) if last.owner_epoch == record.owner_epoch => *last = record,
            _ => owners.push(record),
        }
    }
    Ok(owners)
}

fn journal_unrecognised() -> Outcome {
    Outcome::new(ActingctlCode::WatchdogJournalUnrecognised)
}

/// A journal that is not UTF-8: the decoder's message as `foreign_data_error`.
fn not_utf8(error: &std::str::Utf8Error) -> Outcome {
    journal_unrecognised().caused_by(
        Outcome::new(ContractCode::ForeignDataError)
            .raw_source(RawSource::Other)
            .raw_text(error.to_string()),
    )
}

/// The owner record of one journal line: a v1 or v2 record, or a checkpoint's `last_record`.
/// Another schema is unrecognised with its `declared_schema`; a line without one, or a record
/// that lacks a required field, is unrecognised.
fn owner_line(value: &Value) -> Result<OwnerRecord, Outcome> {
    let value = match value["schema_version"].as_str() {
        Some(OWNER_SCHEMA_V1 | OWNER_SCHEMA_V2) => value,
        Some(OWNER_CHECKPOINT_SCHEMA) => &value["last_record"],
        Some(other) => return Err(journal_unrecognised().declared_schema(other)),
        None => return Err(journal_unrecognised()),
    };
    owner_record(value).ok_or_else(journal_unrecognised)
}

/// Workflow #381 A R5b: among the `LOOKBACK_OWNERS` owners before the last one, the newest whose
/// epoch a candidate log covers (modified from its start to its close plus the clock slack; an
/// owner without a close ends where the next one starts), and the FATAL line of the newest such
/// log, if it holds one.
fn earlier_owner_fatal(
    log_directories: &[PathBuf],
    owners: &[OwnerRecord],
) -> Result<Option<Fatal>, Outcome> {
    let Some((last, earlier)) = owners.split_last() else {
        return Ok(None);
    };
    let logs = candidate_logs(log_directories)?;
    let mut next_started = last.started_at_unix_ms;
    for owner in earlier.iter().rev().take(LOOKBACK_OWNERS) {
        let end = owner
            .closed_at_unix_ms
            .unwrap_or(next_started)
            .saturating_add(super::decide::CLOCK_SLACK_MS);
        // The candidate logs come newest first.
        if let Some((path, _)) = logs
            .iter()
            .find(|(_, modified)| *modified >= owner.started_at_unix_ms && *modified <= end)
        {
            let tail = read_tail(path)?;
            return Ok(fatal_line(&tail).map(|line| Fatal {
                log: plain_path(path).display().to_string(),
                line,
            }));
        }
        next_started = owner.started_at_unix_ms;
    }
    Ok(None)
}

fn unrecognised(failure: Outcome) -> Journal {
    Journal::Unrecognised { failure }
}

fn owner_record(value: &Value) -> Option<OwnerRecord> {
    let schema_version = value["schema_version"].as_str()?;
    if !matches!(schema_version, OWNER_SCHEMA_V1 | OWNER_SCHEMA_V2) {
        return None;
    }
    Some(OwnerRecord {
        schema_version: schema_version.to_owned(),
        owner_epoch: epoch_text(&value["owner_epoch"])?,
        pid: u32::try_from(value["pid"].as_u64()?).ok()?,
        started_at_unix_ms: value["started_at_unix_ms"].as_u64()?,
        active: value["active"].as_bool()?,
        closed_at_unix_ms: value["closed_at_unix_ms"].as_u64(),
        resource_disposition: value["resource_disposition"].as_str().map(str::to_owned),
    })
}

fn epoch_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Null => None,
        other => Some(other.to_string()),
    }
}

/// L, J and F of one state root, the owner observation the decision rows read; `live` is left to
/// the caller. F is searched only for an unlocked journal with a record, among the logs in
/// `log_directories` (Workflow #381 A: a state root a test wrote is read the same way, I7).
pub(crate) fn observe_owner(
    state_root: &Path,
    log_directories: &[PathBuf],
) -> Result<Observed, Outcome> {
    let (lock, journal, bytes) = read_owner_journal(state_root)?;
    let fatal = match (&lock, &journal) {
        (
            OwnerLock::Unlocked,
            Journal::Record {
                modified_unix_ms, ..
            },
        ) => fatal_after(
            log_directories,
            modified_unix_ms.saturating_sub(super::decide::CLOCK_SLACK_MS),
        )?,
        _ => None,
    };
    // R5b: a formal close that no log covers looks back to the owners before it.
    let earlier_fatal = match (&lock, &journal) {
        (OwnerLock::Unlocked, Journal::Record { record, .. })
            if fatal.is_none() && !record.active =>
        {
            let logged = match record.closed_at_unix_ms {
                Some(closed_at) => {
                    close_logged(log_directories, record.started_at_unix_ms, closed_at)?
                }
                None => false,
            };
            if logged {
                None
            } else {
                let owners = owner_blocks(&bytes)
                    .map_err(|failure| failure.path(state_root.join(OWNER_JOURNAL_FILE)))?;
                earlier_owner_fatal(log_directories, &owners)?
            }
        }
        _ => None,
    };
    Ok(Observed {
        lock,
        live: None,
        journal,
        fatal,
        earlier_fatal,
    })
}

/// I, parsed tolerantly for reporting; `None` when absent or unreadable.
pub(crate) fn runtime_info(state_root: &Path) -> Option<Value> {
    let mut bytes = Vec::new();
    File::open(state_root.join(RUNTIME_INFO_FILE))
        .ok()?
        .take(MAX_RUNTIME_INFO_BYTES)
        .read_to_end(&mut bytes)
        .ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    Some(serde_json::json!({
        "pid": value["pid"],
        "owner_epoch": value["owner_epoch"],
        "started_at_unix_ms": value["started_at_unix_ms"],
    }))
}

/// Review M1: alive means the owner named by runtime-info answers a health request; an owner
/// that does not is the client's failure (`RuntimeClientError::outcome`).
pub(crate) fn live_owner(
    state_root: &Path,
    timeout: Option<Duration>,
) -> Result<LiveOwner, Outcome> {
    let config = RuntimeClientConfig::new(state_root, EventActor::Cli, EventSource::Cli)
        .with_io_timeout(timeout.unwrap_or(HEALTH_TIMEOUT));
    let client = RuntimeClient::connect(config).map_err(|error| error.outcome())?;
    let info = client.runtime_info();
    Ok(LiveOwner {
        pid: info.pid(),
        owner_epoch: epoch_text(&serde_json::to_value(info.owner_epoch()).unwrap_or_default())
            .unwrap_or_default(),
        started_at_unix_ms: info.started_at_unix_ms(),
    })
}

/// The logs a FATAL is searched in: `actingd-*.log` in the given directories (the root for
/// acsetup, a logon script and an operator's restart script; `<root>\watchdog` for the
/// watchdog's own starts; the console's log directory).
fn candidate_logs(directories: &[PathBuf]) -> Result<Vec<(PathBuf, u64)>, Outcome> {
    let mut logs = Vec::new();
    for directory in directories {
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(io_failure(
                    ActingctlCode::WatchdogLogDirectoryUnreadable,
                    directory,
                    &error,
                    IoOp::ReadDir,
                ));
            }
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if !(name.starts_with("actingd-") && name.ends_with(".log")) {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            let modified = metadata.modified().map(unix_ms).unwrap_or(0);
            logs.push((entry.path(), modified));
        }
    }
    logs.sort_by_key(|(_, modified)| std::cmp::Reverse(*modified));
    Ok(logs)
}

/// F: the newest candidate log modified at or after `since_unix_ms` whose tail holds a FATAL
/// line of actingd or acforward, other than a lost start race (`owner_conflict`).
pub(crate) fn fatal_after(
    directories: &[PathBuf],
    since_unix_ms: u64,
) -> Result<Option<Fatal>, Outcome> {
    for (path, modified) in candidate_logs(directories)? {
        if modified < since_unix_ms {
            break;
        }
        let tail = read_tail(&path)?;
        if let Some(line) = fatal_line(&tail) {
            return Ok(Some(Fatal {
                log: plain_path(&path).display().to_string(),
                line,
            }));
        }
    }
    Ok(None)
}

/// Review M2 (ii): a formal close is `logged` when some candidate log was written during the
/// closed epoch (actingd prints its ready line after it takes the owner lock).
pub(crate) fn close_logged(
    directories: &[PathBuf],
    started_at_unix_ms: u64,
    closed_at_unix_ms: u64,
) -> Result<bool, Outcome> {
    Ok(candidate_logs(directories)?.iter().any(|(_, modified)| {
        *modified >= started_at_unix_ms
            && *modified <= closed_at_unix_ms.saturating_add(super::decide::CLOCK_SLACK_MS)
    }))
}

/// A log's tail; an unreadable log is `watchdog_log_unreadable` (`path`).
fn read_tail(path: &Path) -> Result<String, Outcome> {
    let unreadable = |error: &std::io::Error, op: IoOp| {
        io_failure(ActingctlCode::WatchdogLogUnreadable, path, error, op)
    };
    let mut file = File::open(path).map_err(|error| unreadable(&error, IoOp::Open))?;
    let length = file
        .metadata()
        .map_err(|error| unreadable(&error, IoOp::Metadata))?
        .len();
    file.seek(SeekFrom::Start(length.saturating_sub(LOG_TAIL_BYTES)))
        .map_err(|error| unreadable(&error, IoOp::Seek))?;
    let mut bytes = Vec::new();
    file.take(LOG_TAIL_BYTES)
        .read_to_end(&mut bytes)
        .map_err(|error| unreadable(&error, IoOp::Read))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// The last FATAL line of actingd or acforward in a log tail; a multi-line detail follows it
/// (review L2), and both print FATAL only as their last act.
pub(crate) fn fatal_line(tail: &str) -> Option<String> {
    let line = tail.lines().rev().find(|line| {
        let line = line.trim_start();
        line.starts_with("FATAL actingd:") || line.starts_with("FATAL acforward:")
    })?;
    (!line.contains(actingcommand_contract::RuntimeErrorCode::OwnerConflict.as_str()))
        .then(|| line.trim().to_owned())
}

/// W, probed only when a start is considered. A free lock is returned held shared, so acsetup
/// cannot begin until this tick ends (review L4); a missing file means acsetup never ran.
pub(crate) enum WriterProbe {
    Free(Option<File>),
    Busy,
}

pub(crate) fn writer_lock(root: &Path) -> Result<WriterProbe, Outcome> {
    let path = root.join(WRITER_LOCK);
    let failed = |error: &std::io::Error, op: IoOp| {
        io_failure(ActingctlCode::WatchdogWriterLockFailed, &path, error, op)
    };
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(WriterProbe::Free(None)),
        Err(error) => return Err(failed(&error, IoOp::Open)),
    };
    match file.try_lock_shared() {
        Ok(()) => Ok(WriterProbe::Free(Some(file))),
        Err(std::fs::TryLockError::WouldBlock) => Ok(WriterProbe::Busy),
        Err(std::fs::TryLockError::Error(error)) => Err(failed(&error, IoOp::Lock)),
    }
}

/// The console starts actingd with its log in `%LOCALAPPDATA%\ActingCommand\logs`; the task runs
/// as the same user.
pub(crate) fn console_log_directory() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .filter(|directory| !directory.is_empty())
        .map(|directory| PathBuf::from(directory).join("ActingCommand").join("logs"))
}

/// What the start rows read from the machine in one hidden PowerShell: P (review H2), the
/// processes named `actingcommand-actingd.exe` whose executable is under the root or whose
/// path cannot be read, and the last boot time (for the gap grace).
pub(crate) struct HostProbe {
    pub(crate) processes: Vec<RuntimeProcess>,
    pub(crate) boot_unix_ms: Option<u64>,
}

/// Every failure is `process_probe_failed`: caused by PowerShell's failure, or by
/// `foreign_data_error` when its output is not JSON; output without `boot_unix_ms`, or a
/// process row without a pid, is the code alone.
pub(crate) fn host_probe(root_plain: &Path) -> Result<HostProbe, Outcome> {
    const SCRIPT: &str = "$ErrorActionPreference = 'Stop'\n\
        $ProgressPreference = 'SilentlyContinue'\n\
        $found = @(Get-CimInstance -ClassName Win32_Process -Filter \"Name='actingcommand-actingd.exe'\" | ForEach-Object { [pscustomobject]@{ pid = [uint32]$_.ProcessId; path = $_.ExecutablePath } })\n\
        $boot = ([DateTimeOffset](Get-CimInstance -ClassName Win32_OperatingSystem).LastBootUpTime).ToUnixTimeMilliseconds()\n\
        [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false\n\
        [Console]::Out.Write((ConvertTo-Json -InputObject ([pscustomobject]@{ boot_unix_ms = $boot; processes = $found }) -Compress -Depth 4))\n";
    let output = powershell::run(
        SCRIPT,
        &[],
        Duration::from_secs(60),
        ActingctlCode::ProcessProbeFailed,
    )?;
    parse_host_probe(&output, root_plain)
}

pub(crate) fn parse_host_probe(output: &str, root_plain: &Path) -> Result<HostProbe, Outcome> {
    let value: Value = serde_json::from_str(output.trim()).map_err(|error| {
        Outcome::new(ActingctlCode::ProcessProbeFailed).caused_by(json_failure(&error))
    })?;
    let boot_unix_ms = value["boot_unix_ms"]
        .as_u64()
        .ok_or_else(|| Outcome::new(ActingctlCode::ProcessProbeFailed))?;
    Ok(HostProbe {
        processes: parse_processes(&value["processes"], root_plain)?,
        boot_unix_ms: Some(boot_unix_ms),
    })
}

pub(crate) fn parse_processes(
    value: &Value,
    root_plain: &Path,
) -> Result<Vec<RuntimeProcess>, Outcome> {
    let rows = match value {
        Value::Array(rows) => rows.clone(),
        Value::Null => Vec::new(),
        row => vec![row.clone()],
    };
    let prefix = format!(
        "{}\\",
        root_plain
            .display()
            .to_string()
            .trim_end_matches('\\')
            .to_lowercase()
    );
    rows.iter()
        .filter_map(|row| {
            let pid = row["pid"].as_u64().and_then(|pid| u32::try_from(pid).ok());
            let path = row["path"].as_str().map(str::to_owned);
            match (pid, path) {
                (None, _) => Some(Err(Outcome::new(ActingctlCode::ProcessProbeFailed))),
                (Some(pid), None) => Some(Ok(RuntimeProcess { pid, path: None })),
                (Some(pid), Some(path)) => {
                    let under_root = path.to_lowercase().starts_with(&prefix);
                    under_root.then_some(Ok(RuntimeProcess {
                        pid,
                        path: Some(path),
                    }))
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const V1: &str = r#"{"schema_version":"actingcommand.runtime-owner.v1","revision":1,"owner_epoch":"epoch_one","pid":10,"started_at_unix_ms":100,"active":false,"active_instances":[],"closed_at_unix_ms":200}"#;
    const V2: &str = r#"{"schema_version":"actingcommand.runtime-owner.v2","revision":2,"owner_epoch":"epoch_two","pid":11,"started_at_unix_ms":300,"active":true,"active_instances":[],"closed_at_unix_ms":null,"resource_disposition":"in_use"}"#;

    fn record(journal: &Journal) -> &OwnerRecord {
        match journal {
            Journal::Record { record, .. } => record,
            other => panic!("expected a record, got {other:?}"),
        }
    }

    #[test]
    fn journals_of_every_shape_give_their_last_record() {
        // v1 only.
        let journal = parse_journal(format!("{V1}\n").as_bytes(), 5);
        let last = record(&journal);
        assert_eq!(
            (
                last.owner_epoch.as_str(),
                last.active,
                last.closed_at_unix_ms
            ),
            ("epoch_one", false, Some(200))
        );
        assert_eq!(last.resource_disposition, None);
        // v1 then v2.
        let journal = parse_journal(format!("{V1}\n{V2}\n").as_bytes(), 5);
        let last = record(&journal);
        assert_eq!((last.pid, last.active), (11, true));
        assert_eq!(last.resource_disposition.as_deref(), Some("in_use"));
        // A checkpoint first, alone: its `last_record`.
        let checkpoint = format!(
            r#"{{"schema_version":"actingcommand.runtime-owner-checkpoint.v1","predecessor_bytes":1,"predecessor_sha256":"{}","last_record":{V2},"epochs":[],"prefixes":[]}}"#,
            "0".repeat(64)
        );
        let journal = parse_journal(format!("{checkpoint}\n").as_bytes(), 5);
        assert_eq!(record(&journal).owner_epoch, "epoch_two");
        // A checkpoint followed by records: the physical last record.
        let journal = parse_journal(format!("{checkpoint}\n{V1}\n").as_bytes(), 5);
        assert_eq!(record(&journal).owner_epoch, "epoch_one");
        // A torn tail is ignored; a torn only line is no record yet.
        let journal = parse_journal(format!("{V1}\n{{\"schema_ver").as_bytes(), 5);
        assert_eq!(record(&journal).owner_epoch, "epoch_one");
        assert_eq!(parse_journal(b"{\"schema", 5), Journal::Absent);
        assert_eq!(parse_journal(b"", 5), Journal::Absent);
        // An unknown schema is unrecognised.
        let journal = parse_journal(
            b"{\"schema_version\":\"actingcommand.runtime-owner.v9\"}\n",
            5,
        );
        assert!(matches!(journal, Journal::Unrecognised { .. }));
    }

    /// Workflow #389 TU-44 (unified interface model v2.2.3, section 8.2; section 3.6): the
    /// watchdog reads a close that carries the upgrade-state marker as usual, as the last
    /// record and as a checkpoint's `last_record`, with the owner record schema still v2.
    #[test]
    fn tu44_the_watchdog_reads_a_close_with_the_upgrade_state_marker_as_usual() {
        const STARTED: &str = r#"{"schema_version":"actingcommand.runtime-owner.v2","revision":3,"owner_epoch":"epoch_three","pid":12,"started_at_unix_ms":400,"active":true,"active_instances":[],"closed_at_unix_ms":null,"resource_disposition":"none"}"#;
        const CLOSED: &str = r#"{"schema_version":"actingcommand.runtime-owner.v2","revision":4,"owner_epoch":"epoch_three","pid":12,"started_at_unix_ms":400,"active":false,"active_instances":[],"closed_at_unix_ms":500,"resource_disposition":"none"}"#;
        const MARKED: &str = r#"{"schema_version":"actingcommand.runtime-owner.v2","revision":4,"owner_epoch":"epoch_three","pid":12,"started_at_unix_ms":400,"active":false,"active_instances":[],"closed_at_unix_ms":500,"resource_disposition":"none","closed_by":"upgrade_state"}"#;
        const NEXT: &str = r#"{"schema_version":"actingcommand.runtime-owner.v2","revision":5,"owner_epoch":"epoch_four","pid":13,"started_at_unix_ms":600,"active":true,"active_instances":[],"closed_at_unix_ms":null,"resource_disposition":"none"}"#;
        let checkpoint = |last_record: &str| {
            format!(
                r#"{{"schema_version":"actingcommand.runtime-owner-checkpoint.v1","predecessor_bytes":1,"predecessor_sha256":"{}","last_record":{last_record},"epochs":[],"prefixes":[]}}"#,
                "0".repeat(64)
            )
        };
        let closed = OwnerRecord {
            schema_version: "actingcommand.runtime-owner.v2".to_owned(),
            owner_epoch: "epoch_three".to_owned(),
            pid: 12,
            started_at_unix_ms: 400,
            active: false,
            closed_at_unix_ms: Some(500),
            resource_disposition: Some("none".to_owned()),
        };

        // The upgrade state's close is the last record.
        let marked = format!("{STARTED}\n{MARKED}\n");
        let unmarked = format!("{STARTED}\n{CLOSED}\n");
        let journal = parse_journal(marked.as_bytes(), 5);
        assert_eq!(record(&journal), &closed);
        assert_eq!(journal, parse_journal(unmarked.as_bytes(), 5));
        assert_eq!(owner_blocks(marked.as_bytes()), Ok(vec![closed.clone()]));

        // The next owner folded it into its checkpoint's `last_record`.
        let marked = format!("{}\n{NEXT}\n", checkpoint(MARKED));
        let unmarked = format!("{}\n{NEXT}\n", checkpoint(CLOSED));
        let owners = owner_blocks(marked.as_bytes()).expect("owners of a folded journal");
        assert_eq!(owners.first(), Some(&closed));
        assert_eq!(
            Ok(owners),
            owner_blocks(unmarked.as_bytes()),
            "the marker changes nothing the watchdog reads"
        );
    }

    #[test]
    fn fatal_lines_skip_a_lost_start_race_and_survive_a_multi_line_detail() {
        assert_eq!(
            fatal_line("actingd ready pid=1\nFATAL actingd: config: bind_port_invalid\n"),
            Some("FATAL actingd: config: bind_port_invalid".to_owned())
        );
        assert_eq!(
            fatal_line("FATAL actingd: runtime: detail first line\nsecond line\n\n"),
            Some("FATAL actingd: runtime: detail first line".to_owned())
        );
        assert_eq!(
            fatal_line("FATAL acforward: Cannot run x\n"),
            Some("FATAL acforward: Cannot run x".to_owned())
        );
        assert_eq!(fatal_line("FATAL actingd: owner_conflict: held\n"), None);
        assert_eq!(fatal_line("actingd ready pid=1\n"), None);
        assert_eq!(fatal_line("FATAL actingctl: usage\n"), None);
    }

    #[test]
    fn processes_count_under_the_root_or_without_a_path() {
        let root = Path::new(r"C:\Install");
        let probe = |text: &str| parse_host_probe(text, root);
        let empty = probe(r#"{"boot_unix_ms":5,"processes":[]}"#).expect("empty");
        assert_eq!((empty.processes, empty.boot_unix_ms), (Vec::new(), Some(5)));
        let rows = r#"{"boot_unix_ms":5,"processes":[{"pid":1,"path":"c:\\install\\A\\runtime\\actingcommand-actingd.exe"},{"pid":2,"path":"C:\\Installer\\runtime\\actingcommand-actingd.exe"},{"pid":3,"path":null}]}"#;
        let found = probe(rows).expect("rows").processes;
        assert_eq!(
            found.iter().map(|process| process.pid).collect::<Vec<_>>(),
            [1, 3]
        );
        let single = r#"{"boot_unix_ms":5,"processes":{"pid":4,"path":"C:\\Install\\runtime\\actingcommand-actingd.exe"}}"#;
        assert_eq!(probe(single).expect("row").processes.len(), 1);
        assert!(probe("not json").is_err());
        assert!(probe(r#"{"processes":[]}"#).is_err());
    }

    #[test]
    fn verbatim_paths_become_plain() {
        assert_eq!(
            plain_path(Path::new(r"\\?\F:\Install")),
            PathBuf::from(r"F:\Install")
        );
        assert_eq!(
            plain_path(Path::new(r"\\?\UNC\server\share")),
            PathBuf::from(r"\\server\share")
        );
    }
}
