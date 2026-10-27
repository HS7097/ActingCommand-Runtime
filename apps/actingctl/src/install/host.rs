// SPDX-License-Identifier: AGPL-3.0-only

//! What `actingctl install` reads from the machine, behind one seam (Workflow #421 INST-1): the
//! clock, the running installer and its interface declaration, a work folder, the processes
//! under a root, the programs of a build it runs (check-config, ledger verification, the ledger
//! reader) each within its deadline, the folders start logs lie in, and the test hooks after
//! each journalled step, between the two renames of a swap and before `current.json` is
//! retired. Production is [`SystemHost`]; the tests (`gate_inst1_*`) use fakes with a temporary
//! root.

use super::manifest::{self, Speaks};
use crate::codes::ActingctlCode;
use crate::watchdog::observe;
use actingcommand_contract::outcome::Outcome;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The interface declaration this build carries in its manifests (the same file).
const OWN_DECLARATION: &str =
    include_str!("../../../../distribution/windows/component-interfaces.json");
/// The process probe's deadline (the watchdog's).
const PROBE_TIMEOUT: Duration = Duration::from_secs(60);

/// The running installer, as an operation's journal names it (`installer {commit, path}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Installer {
    /// The commit its own build manifest names; `None` when the program runs outside a build
    /// (a development build).
    pub(crate) commit: Option<String>,
    pub(crate) path: String,
}

/// A process whose image lies under the root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct RunningImage {
    pub(crate) pid: u32,
    pub(crate) path: String,
    /// Whether its command line serves MCP (`mcp-serve`).
    pub(crate) mcp_serve: bool,
}

/// A program that ran to its end: its exit code and what it printed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Ran {
    pub(crate) exit_code: Option<i32>,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

/// A program that did not run to its end.
#[derive(Debug)]
pub(crate) enum NotRan {
    /// It could not be run or waited for: the OS's failure.
    Failed(Outcome),
    /// Still running at its deadline (INST-1b-F fix round 1, review S1 F4). It was left running,
    /// never killed (the stall rule); `pid` is its process id.
    StillRunning { pid: u32 },
}

pub(crate) trait Host {
    /// The wall clock, in Unix milliseconds: operation ids and the `<ms>` of kept names.
    fn now_unix_ms(&self) -> u64;
    /// The running installer.
    fn installer(&self) -> Installer;
    /// The running installer's interface declaration (party I).
    fn installer_speaks(&self) -> Option<Speaks>;
    /// Where `release verify` extracts the zips (a new folder under it per call).
    fn work_dir(&self) -> PathBuf;
    /// The processes whose image lies under `root`.
    fn processes_under(&self, root: &Path) -> Result<Vec<RunningImage>, Outcome>;
    /// Runs `program` with `arguments` to its end, at most `timeout_ms` milliseconds, and
    /// captures what it printed. A program still running then is left running (never killed)
    /// and answers [`NotRan::StillRunning`]; one that did not run, the OS's failure.
    fn run_program(
        &self,
        program: &Path,
        arguments: &[OsString],
        timeout_ms: u64,
    ) -> Result<Ran, NotRan>;
    /// The folders an actingd start of `root` writes its log to.
    fn log_dirs(&self, root: &Path) -> Vec<PathBuf>;
    /// Called after each journalled step of `operation_id` is recorded done. Production never
    /// fails here; a test returns the failure of a crash at that point, and the operation stops
    /// there exactly as if the process had died (the journal and `current.json` stay).
    fn after_step(&self, _operation_id: &str, _step: usize) -> Result<(), Outcome> {
        Ok(())
    }
    /// Called between the two renames of the swap step `step` of `operation_id` (the live file
    /// renamed aside, the part not yet in its place). Production never fails here; a test
    /// returns the failure of a crash at that point.
    fn between_renames(&self, _operation_id: &str, _step: usize) -> Result<(), Outcome> {
        Ok(())
    }
    /// Called when the end of `operation_id` is recorded in its journal and `current.json` is
    /// not yet retired. Production never fails here; a test returns the failure of a crash at
    /// that point.
    fn before_retire(&self, _operation_id: &str) -> Result<(), Outcome> {
        Ok(())
    }
}

/// The machine.
pub(crate) struct SystemHost;

impl Host for SystemHost {
    fn now_unix_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| {
                u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
            })
    }

    fn installer(&self) -> Installer {
        let executable = std::env::current_exe().ok();
        let commit = executable
            .as_deref()
            .and_then(Path::parent)
            .and_then(|directory| manifest::read_manifest(directory).ok())
            .map(|manifest| manifest.commit_sha);
        Installer {
            commit,
            path: executable.map_or_else(String::new, |path| path.display().to_string()),
        }
    }

    fn installer_speaks(&self) -> Option<Speaks> {
        let value: Value = serde_json::from_str(OWN_DECLARATION).ok()?;
        manifest::parse_declaration(&value)
    }

    fn work_dir(&self) -> PathBuf {
        std::env::temp_dir()
    }

    /// The hidden CIM query the watchdog runs, over every process with an image path; the root
    /// travels in an environment variable, never in the script.
    fn processes_under(&self, root: &Path) -> Result<Vec<RunningImage>, Outcome> {
        const SCRIPT: &str = "$ErrorActionPreference = 'Stop'\n\
            $ProgressPreference = 'SilentlyContinue'\n\
            $found = @(Get-CimInstance -ClassName Win32_Process | Where-Object { $_.ExecutablePath } | ForEach-Object { [pscustomobject]@{ pid = [uint32]$_.ProcessId; path = $_.ExecutablePath; mcp = ([string]$_.CommandLine).Contains('mcp-serve') } })\n\
            [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false\n\
            [Console]::Out.Write((ConvertTo-Json -InputObject $found -Compress -Depth 3))\n";
        let output = crate::watchdog::powershell::run(
            SCRIPT,
            &[],
            PROBE_TIMEOUT,
            ActingctlCode::ProcessProbeFailed,
        )?;
        parse_processes(&output, root)
    }

    fn run_program(
        &self,
        program: &Path,
        arguments: &[OsString],
        timeout_ms: u64,
    ) -> Result<Ran, NotRan> {
        let mut command = Command::new(program);
        command.args(arguments);
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        // A child still running at the deadline keeps running (`child` never kills one). A
        // reader thread that panicked leaves no OS failure: the program could not be run.
        let captured =
            crate::child::run_captured(command, deadline).map_err(|failure| match failure {
                crate::child::ChildFailure::StillRunning { pid } => NotRan::StillRunning { pid },
                failure => NotRan::Failed(
                    failure
                        .os_failure()
                        .map(|cause| cause.path(program))
                        .unwrap_or_else(|| Outcome::new(ActingctlCode::SpawnFailed).path(program)),
                ),
            })?;
        Ok(Ran {
            exit_code: captured.status.code(),
            stdout: captured.stdout,
            stderr: captured.stderr,
        })
    }

    fn log_dirs(&self, root: &Path) -> Vec<PathBuf> {
        let mut directories = vec![root.to_path_buf(), root.join("watchdog")];
        directories.extend(observe::console_log_directory());
        directories
    }
}

/// The probe's rows whose image lies under `root` (case-insensitive, by the root's plain
/// spelling); a row without a pid is the probe's failure.
pub(crate) fn parse_processes(output: &str, root: &Path) -> Result<Vec<RunningImage>, Outcome> {
    let failed = || Outcome::new(ActingctlCode::ProcessProbeFailed);
    let value: Value = if output.trim().is_empty() {
        Value::Array(Vec::new())
    } else {
        serde_json::from_str(output.trim())
            .map_err(|error| failed().caused_by(super::files::json_failure(&error)))?
    };
    let rows = match value {
        Value::Array(rows) => rows,
        Value::Null => Vec::new(),
        row => vec![row],
    };
    let prefix = format!(
        "{}\\",
        super::files::plain(root)
            .trim_end_matches('\\')
            .to_lowercase()
    );
    let mut images = Vec::new();
    for row in rows {
        let pid = row["pid"]
            .as_u64()
            .and_then(|pid| u32::try_from(pid).ok())
            .ok_or_else(failed)?;
        let Some(path) = row["path"].as_str() else {
            continue;
        };
        if path.to_lowercase().starts_with(&prefix) {
            images.push(RunningImage {
                pid,
                path: path.to_owned(),
                mcp_serve: row["mcp"].as_bool().unwrap_or(false),
            });
        }
    }
    Ok(images)
}
