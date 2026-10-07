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

use super::decide::{CLOCK_SLACK_MS, LiveOwner, OwnerLock, READY_TIMEOUT_MS};
use super::observe::{self, Installation, RUNTIME_IMAGE};
use super::powershell::{self, CREATE_NO_WINDOW};
use crate::process_probe::pid_alive;
use actingcommand_contract::{INSTALL_ROOT_ENV, INSTALL_SELECTION_ENV};
use serde_json::Value;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::os::windows::process::CommandExt;
use std::path::Path;
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

pub(crate) struct StartError {
    pub(crate) code: &'static str,
    pub(crate) detail: String,
}

pub(crate) struct Outcome {
    pub(crate) method: &'static str,
    pub(crate) result: Result<LiveOwner, StartError>,
}

enum Started {
    /// The breakaway child (the fixed entry); its exit ends the start.
    Child(Child),
    /// The `cmd` that WMI created; it lives as long as the fixed entry it runs.
    Pid(u32),
}

fn failed(method: &'static str, code: &'static str, detail: String) -> Outcome {
    Outcome {
        method,
        result: Err(StartError { code, detail }),
    }
}

pub(crate) fn start(
    installation: &Installation,
    log: &Path,
    spawn_at_unix_ms: u64,
    from_task: bool,
) -> Outcome {
    let entry = installation.entry(RUNTIME_IMAGE);
    if !entry.is_file() {
        let method = if from_task { "breakaway" } else { "wmi" };
        return failed(
            method,
            "entry_missing",
            format!("{} is not a file", entry.display()),
        );
    }
    let file = match OpenOptions::new().write(true).create_new(true).open(log) {
        Ok(file) => file,
        Err(error) => {
            return failed(
                if from_task { "breakaway" } else { "wmi" },
                "log_create_failed",
                format!("{}: {error}", log.display()),
            );
        }
    };
    let (method, started) = if from_task {
        match spawn_breakaway(installation, &entry, file) {
            Ok(child) => ("breakaway", Started::Child(child)),
            Err(error) if error.raw_os_error() == Some(ACCESS_DENIED) => {
                match spawn_wmi(installation, &entry, log) {
                    Ok(pid) => ("wmi", Started::Pid(pid)),
                    Err(error) => return failed("wmi", error.code, error.detail),
                }
            }
            Err(error) => {
                return failed(
                    "breakaway",
                    "spawn_failed",
                    format!("{}: {error}", entry.display()),
                );
            }
        }
    } else {
        drop(file);
        match spawn_wmi(installation, &entry, log) {
            Ok(pid) => ("wmi", Started::Pid(pid)),
            Err(error) => return failed("wmi", error.code, error.detail),
        }
    };
    Outcome {
        method,
        result: wait_ready(installation, started, log, spawn_at_unix_ms),
    }
}

fn spawn_breakaway(installation: &Installation, entry: &Path, log: File) -> std::io::Result<Child> {
    let stderr = log.try_clone()?;
    Command::new(entry)
        .arg("--config")
        .arg(&installation.config_plain)
        .current_dir(&installation.root_plain)
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
/// `cmd /d /s /c` so stdout and stderr are appended to the log.
fn spawn_wmi(installation: &Installation, entry: &Path, log: &Path) -> Result<u32, StartError> {
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
        ("AC_WD_CONFIG", installation.config_plain.as_path()),
        ("AC_WD_LOG", log.as_path()),
        ("AC_WD_ROOT", installation.root_plain.as_path()),
    ];
    if let Some((_, unsafe_path)) = paths
        .iter()
        .find(|(_, path)| path.to_string_lossy().contains(CMD_UNSAFE))
    {
        return Err(StartError {
            code: "watchdog_wmi_path_unsafe",
            detail: format!(
                "{} holds a cmd metacharacter ({}); start the Runtime formally",
                unsafe_path.display(),
                CMD_UNSAFE.iter().collect::<String>()
            ),
        });
    }
    let environment = paths
        .iter()
        .map(|(name, path)| (*name, path.as_os_str()))
        .collect::<Vec<_>>();
    let output =
        powershell::run(SCRIPT, &environment, Duration::from_secs(60)).map_err(|detail| {
            StartError {
                code: "wmi_start_failed",
                detail,
            }
        })?;
    let value: Value = serde_json::from_str(output.trim()).map_err(|error| StartError {
        code: "wmi_start_failed",
        detail: format!("unexpected output {output:?}: {error}"),
    })?;
    match (value["return_value"].as_i64(), value["pid"].as_u64()) {
        (Some(0), Some(pid)) if pid > 0 => u32::try_from(pid).map_err(|_| StartError {
            code: "wmi_start_failed",
            detail: format!("pid {pid} out of range"),
        }),
        _ => Err(StartError {
            code: "wmi_start_failed",
            detail: format!("Win32_Process.Create returned {value}"),
        }),
    }
}

/// Ready when the owner lock is held and runtime-info names an owner that started after the
/// spawn and answers. The process is never terminated here, like acsetup's own start.
fn wait_ready(
    installation: &Installation,
    mut started: Started,
    log: &Path,
    spawn_at_unix_ms: u64,
) -> Result<LiveOwner, StartError> {
    let deadline = Instant::now() + Duration::from_millis(READY_TIMEOUT_MS);
    loop {
        if let Ok((OwnerLock::Locked, _)) = observe::owner_journal(&installation.state_root)
            && let Ok(owner) = observe::live_owner(&installation.state_root, Some(HEALTH_TIMEOUT))
            && owner.started_at_unix_ms >= spawn_at_unix_ms.saturating_sub(CLOCK_SLACK_MS)
        {
            return Ok(owner);
        }
        let exited = match &mut started {
            Started::Child(child) => match child.try_wait() {
                Ok(Some(status)) => Some(status.to_string()),
                Ok(None) => None,
                Err(error) => Some(format!("state unavailable: {error}")),
            },
            Started::Pid(pid) => match pid_alive(*pid) {
                Ok(true) => None,
                Ok(false) => Some(format!("cmd pid {pid} ended")),
                Err(error) => {
                    return Err(StartError {
                        code: "process_probe_failed",
                        detail: error,
                    });
                }
            },
        };
        if let Some(exit) = exited {
            return Err(StartError {
                code: "exited_during_startup",
                detail: format!("{exit}; {}", last_log_line(log)),
            });
        }
        if Instant::now() >= deadline {
            return Err(StartError {
                code: "start_timeout",
                detail: format!(
                    "no answering owner within {} s; the process was left running; {}",
                    READY_TIMEOUT_MS / 1000,
                    last_log_line(log)
                ),
            });
        }
        thread::sleep(POLL_INTERVAL);
    }
}

/// The log's FATAL line, else its last non-empty line.
fn last_log_line(log: &Path) -> String {
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
