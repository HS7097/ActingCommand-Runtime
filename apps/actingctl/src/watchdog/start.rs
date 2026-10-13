// SPDX-License-Identifier: AGPL-3.0-only

//! Row 7f: starting the Runtime through the fixed entry `<root>\runtime\actingcommand-actingd.exe`
//! with a hidden window, outside the caller's job, with stdout and stderr in a log (X3).
//!
//! - A task tick (`--from-task`) uses CreateProcess with `CREATE_NO_WINDOW |
//!   CREATE_BREAKAWAY_FROM_JOB`; only when the job denies breakaway (os error 5) does it fall
//!   back to WMI. Its stdio is NUL and the watchdog log, so nothing it inherits can hold a
//!   caller (review M4).
//! - A manual run uses WMI `Win32_Process.Create` only: no handle of the caller's terminal
//!   reaches the Runtime, which is never started inside the caller's job.
//!
//! The installation selection travels once: the fixed entry gets no inherited selection and
//! `--config` names the selected generation's config, read again under the shared writer lock
//! (review M3). Both methods run the Runtime at normal priority (review M5).
//!
//! Workflow #389 MCP-10: `actingctl start` and `restart` start through the same manual method,
//! for an A/B root's selection or for a verification root's explicit config ([`Target`]).
//!
//! Workflow #378 A5: a failed start is one outcome: `entry_missing`, `log_create_failed`,
//! `spawn_failed`, `watchdog_wmi_path_unsafe`, `wmi_start_failed`, `process_probe_failed`,
//! `exited_during_startup` or `start_timeout`, with its values and the OS's or PowerShell's
//! failure as its link.

use super::decide::{CLOCK_SLACK_MS, LiveOwner, OwnerLock, READY_TIMEOUT_MS};
use super::observe::{self, Installation, RUNTIME_IMAGE};
use super::powershell::{self, CREATE_NO_WINDOW};
use super::{io_failure, json_failure};
use crate::codes::ActingctlCode;
use crate::process_probe::pid_alive;
use actingcommand_contract::outcome::{IoOp, Outcome};
use actingcommand_contract::{INSTALL_ROOT_ENV, INSTALL_SELECTION_ENV};
use serde_json::Value;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
const NORMAL_PRIORITY_CLASS: u32 = 0x0000_0020;
const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
const ACCESS_DENIED: i32 = 5;
const POLL_INTERVAL: Duration = Duration::from_millis(500);
const HEALTH_TIMEOUT: Duration = Duration::from_secs(2);
/// cmd metacharacters that a quoted path still cannot carry safely.
const CMD_UNSAFE: &[char] = &['&', '|', '<', '>', '^', '%', '!', '"'];

/// One start: the method it used and the answering owner, or the start's failure.
pub(crate) struct Attempt {
    pub(crate) method: &'static str,
    pub(crate) result: Result<LiveOwner, Outcome>,
}

enum Started {
    /// The breakaway child (the fixed entry); its exit ends the start.
    Child(Child),
    /// The `cmd` that WMI created; it lives as long as the fixed entry it runs.
    Pid(u32),
}

fn failed(method: &'static str, failure: Outcome) -> Attempt {
    Attempt {
        method,
        result: Err(failure),
    }
}

// --- Workflow #389 MCP-10 (begin): the start's inputs, shared with `actingctl start`. ---
/// What one start needs: the root (working directory, fixed entry), the config handed to the
/// fixed entry, the state root whose owner answers, and how long readiness may take.
pub(crate) struct Target<'a> {
    pub(crate) root_plain: &'a Path,
    pub(crate) config_plain: &'a Path,
    pub(crate) state_root: &'a Path,
    pub(crate) ready_timeout: Duration,
}

impl<'a> Target<'a> {
    /// The tick's target: the installation's selection and the fixed readiness window.
    pub(crate) fn of(installation: &'a Installation) -> Self {
        Self {
            root_plain: &installation.root_plain,
            config_plain: &installation.config_plain,
            state_root: &installation.state_root,
            ready_timeout: Duration::from_millis(READY_TIMEOUT_MS),
        }
    }

    pub(crate) fn entry(&self) -> PathBuf {
        self.root_plain.join("runtime").join(RUNTIME_IMAGE)
    }
}

/// The checks of a manual start that need no spawn, so `restart` refuses before it stops
/// anything: the fixed entry is a file (`entry_missing`), and no path the WMI start carries
/// holds a cmd metacharacter (`watchdog_wmi_path_unsafe`; start the Runtime formally).
pub(crate) fn preflight(target: &Target<'_>) -> Result<(), Outcome> {
    let entry = target.entry();
    if !entry.is_file() {
        return Err(Outcome::new(ActingctlCode::EntryMissing).path(&entry));
    }
    match [entry.as_path(), target.config_plain, target.root_plain]
        .into_iter()
        .find(|path| path.to_string_lossy().contains(CMD_UNSAFE))
    {
        Some(unsafe_path) => {
            Err(Outcome::new(ActingctlCode::WatchdogWmiPathUnsafe).path(unsafe_path))
        }
        None => Ok(()),
    }
}

pub(crate) fn start(
    installation: &Installation,
    log: &Path,
    spawn_at_unix_ms: u64,
    from_task: bool,
) -> Attempt {
    start_target(&Target::of(installation), log, spawn_at_unix_ms, from_task)
}
// --- Workflow #389 MCP-10 (end). ---

pub(crate) fn start_target(
    installation: &Target<'_>,
    log: &Path,
    spawn_at_unix_ms: u64,
    from_task: bool,
) -> Attempt {
    let entry = installation.entry();
    let method = if from_task { "breakaway" } else { "wmi" };
    if !entry.is_file() {
        return failed(
            method,
            Outcome::new(ActingctlCode::EntryMissing).path(&entry),
        );
    }
    let file = match OpenOptions::new().write(true).create_new(true).open(log) {
        Ok(file) => file,
        Err(error) => {
            return failed(
                method,
                io_failure(ActingctlCode::LogCreateFailed, log, &error, IoOp::Create),
            );
        }
    };
    let (method, started) = if from_task {
        match spawn_breakaway(installation, &entry, file) {
            Ok(child) => ("breakaway", Started::Child(child)),
            Err(error) if error.raw_os_error() == Some(ACCESS_DENIED) => {
                match spawn_wmi(installation, &entry, log) {
                    Ok(pid) => ("wmi", Started::Pid(pid)),
                    Err(failure) => return failed("wmi", failure),
                }
            }
            Err(error) => {
                return failed(
                    "breakaway",
                    io_failure(ActingctlCode::SpawnFailed, &entry, &error, IoOp::Spawn),
                );
            }
        }
    } else {
        drop(file);
        match spawn_wmi(installation, &entry, log) {
            Ok(pid) => ("wmi", Started::Pid(pid)),
            Err(failure) => return failed("wmi", failure),
        }
    };
    Attempt {
        method,
        result: wait_ready(installation, started, log, spawn_at_unix_ms),
    }
}

fn spawn_breakaway(installation: &Target<'_>, entry: &Path, log: File) -> std::io::Result<Child> {
    let stderr = log.try_clone()?;
    Command::new(entry)
        .arg("--config")
        .arg(installation.config_plain)
        .current_dir(installation.root_plain)
        .env_remove(INSTALL_ROOT_ENV)
        .env_remove(INSTALL_SELECTION_ENV)
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(stderr)
        .creation_flags(
            CREATE_NO_WINDOW
                | CREATE_BREAKAWAY_FROM_JOB
                | CREATE_NEW_PROCESS_GROUP
                | NORMAL_PRIORITY_CLASS,
        )
        .spawn()
}

/// X3 literally: `Win32_Process.Create` with `ShowWindow = 0` and normal priority, through
/// `cmd /d /s /c` so stdout and stderr are appended to the log. A path with a cmd
/// metacharacter is `watchdog_wmi_path_unsafe`; every other failure is `wmi_start_failed`
/// (with `return_value` when `Win32_Process.Create` refused, or `pid` out of range).
fn spawn_wmi(installation: &Target<'_>, entry: &Path, log: &Path) -> Result<u32, Outcome> {
    const SCRIPT: &str = "$ErrorActionPreference = 'Stop'\n\
        $ProgressPreference = 'SilentlyContinue'\n\
        $shell = if ($env:ComSpec) { $env:ComSpec } else { Join-Path $env:SystemRoot 'System32\\cmd.exe' }\n\
        $line = '\"{0}\" /d /s /c \"\"{1}\" --config \"{2}\" 1>>\"{3}\" 2>&1\"' -f $shell, $env:AC_WD_ENTRY, $env:AC_WD_CONFIG, $env:AC_WD_LOG\n\
        $startup = New-CimInstance -ClassName Win32_ProcessStartup -ClientOnly -Property @{ ShowWindow = [uint16]0; PriorityClass = [uint32]32 }\n\
        $result = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = $line; CurrentDirectory = $env:AC_WD_ROOT; ProcessStartupInformation = $startup }\n\
        [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false\n\
        [Console]::Out.Write((ConvertTo-Json -InputObject ([pscustomobject]@{ return_value = [int]$result.ReturnValue; pid = [uint32]$result.ProcessId }) -Compress))\n";
    let log = observe::plain_path(log);
    let paths = [
        ("AC_WD_ENTRY", entry),
        ("AC_WD_CONFIG", installation.config_plain),
        ("AC_WD_LOG", log.as_path()),
        ("AC_WD_ROOT", installation.root_plain),
    ];
    if let Some((_, unsafe_path)) = paths
        .iter()
        .find(|(_, path)| path.to_string_lossy().contains(CMD_UNSAFE))
    {
        return Err(Outcome::new(ActingctlCode::WatchdogWmiPathUnsafe).path(unsafe_path));
    }
    let environment = paths
        .iter()
        .map(|(name, path)| (*name, path.as_os_str()))
        .collect::<Vec<_>>();
    let output = powershell::run(
        SCRIPT,
        &environment,
        Duration::from_secs(60),
        ActingctlCode::WmiStartFailed,
    )?;
    let value: Value = serde_json::from_str(output.trim()).map_err(|error| {
        Outcome::new(ActingctlCode::WmiStartFailed).caused_by(json_failure(&error))
    })?;
    match (value["return_value"].as_i64(), value["pid"].as_u64()) {
        (Some(0), Some(pid)) if pid > 0 => {
            u32::try_from(pid).map_err(|_| Outcome::new(ActingctlCode::WmiStartFailed).pid(pid))
        }
        (Some(return_value), _) => {
            Err(Outcome::new(ActingctlCode::WmiStartFailed).return_value(return_value))
        }
        (None, _) => Err(Outcome::new(ActingctlCode::WmiStartFailed)),
    }
}

/// Ready when the owner lock is held and runtime-info names an owner that started after the
/// spawn and answers. The process is never terminated here, like acsetup's own start. A start
/// that ends first is `exited_during_startup` (`log`, the child's `exit_code` or the cmd's
/// `pid`); one that does not answer in time is `start_timeout` (`log`, `timeout_ms`; the
/// process is left running).
fn wait_ready(
    installation: &Target<'_>,
    mut started: Started,
    log: &Path,
    spawn_at_unix_ms: u64,
) -> Result<LiveOwner, Outcome> {
    let deadline = Instant::now() + installation.ready_timeout;
    let exited = || Outcome::new(ActingctlCode::ExitedDuringStartup).log(log);
    loop {
        if let Ok((OwnerLock::Locked, _)) = observe::owner_journal(installation.state_root)
            && let Ok(owner) = observe::live_owner(installation.state_root, Some(HEALTH_TIMEOUT))
            && owner.started_at_unix_ms >= spawn_at_unix_ms.saturating_sub(CLOCK_SLACK_MS)
        {
            return Ok(owner);
        }
        let ended = match &mut started {
            Started::Child(child) => match child.try_wait() {
                Ok(Some(status)) => Some(match status.code() {
                    Some(exit_code) => exited().exit_code(exit_code),
                    None => exited(),
                }),
                Ok(None) => None,
                // The child's state is unavailable: it no longer counts as starting.
                Err(error) => Some(exited().caused_by(Outcome::from_io_error(&error, IoOp::Wait))),
            },
            Started::Pid(pid) => {
                if pid_alive(*pid)? {
                    None
                } else {
                    Some(exited().pid(*pid))
                }
            }
        };
        if let Some(ended) = ended {
            return Err(ended);
        }
        if Instant::now() >= deadline {
            return Err(Outcome::new(ActingctlCode::StartTimeout)
                .log(log)
                .timeout_ms(installation.ready_timeout));
        }
        thread::sleep(POLL_INTERVAL);
    }
}

/// The log's FATAL line, else its last non-empty line: developer text for the start's failure
/// line in `watchdog.log` and on stderr, never an interface value.
pub(crate) fn last_log_line(log: &Path) -> String {
    let mut tail = Vec::new();
    let read = File::open(log).and_then(|mut file| {
        let length = file.metadata()?.len();
        file.seek(SeekFrom::Start(length.saturating_sub(64 * 1024)))?;
        file.read_to_end(&mut tail)
    });
    if let Err(error) = read {
        return format!("log unreadable: {error}");
    }
    let tail = String::from_utf8_lossy(&tail);
    observe::fatal_line(&tail)
        .or_else(|| {
            tail.lines()
                .rev()
                .find(|line| !line.trim().is_empty())
                .map(|line| line.trim().to_owned())
        })
        .map_or_else(
            || "the log is empty".to_owned(),
            |line| format!("log: {line}"),
        )
}
