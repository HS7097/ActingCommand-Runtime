// SPDX-License-Identifier: AGPL-3.0-only

//! Read-only observation for one tick: the A/B installation, the selected configuration's
//! state root, the owner lock and journal, runtime-info, FATAL logs, acsetup's writer lock and
//! the Runtime processes under the root. Nothing here writes, and the owner journal is never
//! locked.

use super::Failure;
use super::decide::{Fatal, Journal, LiveOwner, OwnerLock, OwnerRecord, RuntimeProcess};
use super::powershell;
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
    pub(crate) fn resolve(root_argument: &Path) -> Result<Self, Failure> {
        let root = fs::canonicalize(root_argument).map_err(|error| {
            Failure::misconfigured(
                "watchdog_root_unavailable",
                format!("{}: {error}", root_argument.display()),
            )
        })?;
        let inherited = actingcommand_contract::process_installation().map_err(|error| {
            Failure::misconfigured(error.code(), "the process installation is unreadable")
        })?;
        let selected = match inherited {
            Some(selected) if selected.root() == root => selected.clone(),
            Some(selected) => {
                return Err(Failure::misconfigured(
                    "watchdog_root_mismatch",
                    format!(
                        "this actingctl belongs to {}, not {}",
                        plain_path(selected.root()).display(),
                        plain_path(&root).display()
                    ),
                ));
            }
            None => match InstalledProcess::read_active(&root) {
                Ok(Some(selected)) => selected,
                Ok(None) => {
                    return Err(Failure::misconfigured(
                        "watchdog_layout_unknown",
                        format!(
                            "{} has no install\\active.json; the watchdog supervises A/B installs only",
                            plain_path(&root).display()
                        ),
                    ));
                }
                Err(error) => {
                    return Err(Failure::misconfigured(
                        error.code(),
                        "install\\active.json or its inputs are unreadable",
                    ));
                }
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

    pub(crate) fn entry(&self, image: &str) -> PathBuf {
        self.root_plain.join("runtime").join(image)
    }

    /// Review M3: `install\active.json` read again under the shared writer lock, so it cannot
    /// change before the fixed entry reads it once more.
    pub(crate) fn selection_unchanged(&self) -> Result<bool, Failure> {
        match InstalledProcess::read_active(&self.root) {
            Ok(Some(current)) => Ok(current.selection_json() == self.selection_json),
            Ok(None) => Ok(false),
            Err(error) => Err(Failure::misconfigured(
                error.code(),
                "install\\active.json is unreadable before the start",
            )),
        }
    }
}

fn selected_state_root(selected: &InstalledProcess) -> Result<PathBuf, Failure> {
    let bytes = selected
        .config_bytes()
        .map_err(|error| Failure::misconfigured(error.code(), "the selected config"))?;
    let document: Value = serde_json::from_slice(&bytes)
        .map_err(|error| Failure::misconfigured("watchdog_config_unreadable", error.to_string()))?;
    let state_root = document
        .get("state_root")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| {
            Failure::misconfigured(
                "watchdog_state_root_invalid",
                "the selected config's state_root must be an absolute path",
            )
        })?;
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
pub(crate) fn owner_journal(state_root: &Path) -> Result<(OwnerLock, Journal), Failure> {
    let path = state_root.join(OWNER_JOURNAL_FILE);
    let unreadable = |detail: String| {
        Failure::misconfigured(
            "watchdog_journal_unreadable",
            format!("{}: {detail}", path.display()),
        )
    };
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok((OwnerLock::Missing, Journal::Absent));
        }
        Err(error) => return Err(unreadable(error.to_string())),
    };
    let modified_unix_ms = file
        .metadata()
        .and_then(|metadata| metadata.modified())
        .map(unix_ms)
        .map_err(|error| unreadable(error.to_string()))?;
    let mut bytes = Vec::new();
    match file.take(OWNER_JOURNAL_LIMIT + 1).read_to_end(&mut bytes) {
        Err(error) if error.kind() == ErrorKind::WouldBlock || error.raw_os_error() == Some(33) => {
            return Ok((OwnerLock::Locked, Journal::Absent));
        }
        Err(error) => return Err(unreadable(error.to_string())),
        Ok(_) if bytes.len() as u64 > OWNER_JOURNAL_LIMIT => {
            return Err(unreadable("larger than the owner journal limit".to_owned()));
        }
        Ok(_) => {}
    }
    Ok((OwnerLock::Unlocked, parse_journal(&bytes, modified_unix_ms)))
}

pub(crate) fn parse_journal(bytes: &[u8], modified_unix_ms: u64) -> Journal {
    let complete = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |end| end + 1);
    let Ok(text) = std::str::from_utf8(&bytes[..complete]) else {
        return unrecognised("the journal is not UTF-8");
    };
    let Some(line) = text.lines().rev().find(|line| !line.trim().is_empty()) else {
        return Journal::Absent;
    };
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return unrecognised("the last record is not JSON");
    };
    let value = match value["schema_version"].as_str() {
        Some(OWNER_SCHEMA_V1 | OWNER_SCHEMA_V2) => &value,
        Some(OWNER_CHECKPOINT_SCHEMA) => &value["last_record"],
        Some(other) => return unrecognised(&format!("schema {other}")),
        None => return unrecognised("the last record has no schema_version"),
    };
    match owner_record(value) {
        Some(record) => Journal::Record {
            record,
            modified_unix_ms,
        },
        None => unrecognised("the last owner record lacks a required field"),
    }
}

fn unrecognised(detail: &str) -> Journal {
    Journal::Unrecognised {
        detail: detail.to_owned(),
    }
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

/// Review M1: alive means the owner named by runtime-info answers a health request.
pub(crate) fn live_owner(
    state_root: &Path,
    timeout: Option<Duration>,
) -> Result<LiveOwner, String> {
    let config = RuntimeClientConfig::new(state_root, EventActor::Cli, EventSource::Cli)
        .with_io_timeout(timeout.unwrap_or(HEALTH_TIMEOUT));
    let client = RuntimeClient::connect(config).map_err(|error| error.to_string())?;
    let info = client.runtime_info();
    Ok(LiveOwner {
        pid: info.pid(),
        owner_epoch: epoch_text(&serde_json::to_value(info.owner_epoch()).unwrap_or_default())
            .unwrap_or_default(),
        started_at_unix_ms: info.started_at_unix_ms(),
    })
}

/// The logs a FATAL is searched in: `actingd-*.log` in the root (acsetup, the logon script,
/// the coordinator's scripts) and in `<root>\watchdog` (the watchdog's own starts).
fn candidate_logs(directories: &[PathBuf]) -> Result<Vec<(PathBuf, u64)>, Failure> {
    let mut logs = Vec::new();
    for directory in directories {
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(Failure::misconfigured(
                    "watchdog_log_directory_unreadable",
                    format!("{}: {error}", directory.display()),
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
) -> Result<Option<Fatal>, Failure> {
    for (path, modified) in candidate_logs(directories)? {
        if modified < since_unix_ms {
            break;
        }
        let tail = read_tail(&path).map_err(|error| {
            Failure::misconfigured(
                "watchdog_log_unreadable",
                format!("{}: {error}", path.display()),
            )
        })?;
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
) -> Result<bool, Failure> {
    Ok(candidate_logs(directories)?.iter().any(|(_, modified)| {
        *modified >= started_at_unix_ms
            && *modified <= closed_at_unix_ms.saturating_add(super::decide::CLOCK_SLACK_MS)
    }))
}

fn read_tail(path: &Path) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    file.seek(SeekFrom::Start(length.saturating_sub(LOG_TAIL_BYTES)))?;
    let mut bytes = Vec::new();
    file.take(LOG_TAIL_BYTES).read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// The last FATAL line of actingd or acforward in a log tail; a multi-line detail follows it
/// (review L2), and both print FATAL only as their last act.
pub(crate) fn fatal_line(tail: &str) -> Option<String> {
    let line = tail.lines().rev().find(|line| {
        let line = line.trim_start();
        line.starts_with("FATAL actingd:") || line.starts_with("FATAL acforward:")
    })?;
    (!line.contains("owner_conflict")).then(|| line.trim().to_owned())
}

/// W, probed only when a start is considered. A free lock is returned held shared, so acsetup
/// cannot begin until this tick ends (review L4); a missing file means acsetup never ran.
pub(crate) enum WriterProbe {
    Free(Option<File>),
    Busy,
}

pub(crate) fn writer_lock(root: &Path) -> Result<WriterProbe, Failure> {
    let path = root.join(WRITER_LOCK);
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(WriterProbe::Free(None)),
        Err(error) => {
            return Err(Failure::misconfigured(
                "watchdog_writer_lock_failed",
                format!("{}: {error}", path.display()),
            ));
        }
    };
    match file.try_lock_shared() {
        Ok(()) => Ok(WriterProbe::Free(Some(file))),
        Err(std::fs::TryLockError::WouldBlock) => Ok(WriterProbe::Busy),
        Err(std::fs::TryLockError::Error(error)) => Err(Failure::misconfigured(
            "watchdog_writer_lock_failed",
            format!("{}: {error}", path.display()),
        )),
    }
}

/// P (review H2): processes named `actingcommand-actingd.exe` whose executable is under the
/// root, or whose path cannot be read. Queried through CIM, only when a start is considered.
pub(crate) fn runtime_processes(root_plain: &Path) -> Result<Vec<RuntimeProcess>, String> {
    const SCRIPT: &str = "$ErrorActionPreference = 'Stop'\n\
        $ProgressPreference = 'SilentlyContinue'\n\
        $found = @(Get-CimInstance -ClassName Win32_Process -Filter \"Name='actingcommand-actingd.exe'\" | ForEach-Object { [pscustomobject]@{ pid = [uint32]$_.ProcessId; path = $_.ExecutablePath } })\n\
        [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false\n\
        [Console]::Out.Write((ConvertTo-Json -InputObject $found -Compress))\n";
    let output = powershell::run(SCRIPT, &[], Duration::from_secs(60))?;
    parse_processes(&output, root_plain)
}

pub(crate) fn parse_processes(
    output: &str,
    root_plain: &Path,
) -> Result<Vec<RuntimeProcess>, String> {
    let text = output.trim();
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let value: Value = serde_json::from_str(text)
        .map_err(|error| format!("process query output is not JSON: {error}"))?;
    let rows = match value {
        Value::Array(rows) => rows,
        Value::Null => Vec::new(),
        row => vec![row],
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
                (None, _) => Some(Err(format!("process row without a pid: {row}"))),
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
        assert_eq!(parse_processes("", root), Ok(Vec::new()));
        assert_eq!(parse_processes("[]", root), Ok(Vec::new()));
        let rows = r#"[{"pid":1,"path":"c:\\install\\A\\runtime\\actingcommand-actingd.exe"},{"pid":2,"path":"C:\\Installer\\runtime\\actingcommand-actingd.exe"},{"pid":3,"path":null}]"#;
        let found = parse_processes(rows, root).expect("rows");
        assert_eq!(
            found.iter().map(|process| process.pid).collect::<Vec<_>>(),
            [1, 3]
        );
        let single = r#"{"pid":4,"path":"C:\\Install\\runtime\\actingcommand-actingd.exe"}"#;
        assert_eq!(parse_processes(single, root).expect("row").len(), 1);
        assert!(parse_processes("not json", root).is_err());
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
