// SPDX-License-Identifier: AGPL-3.0-only

//! Process liveness without unsafe code, shared by `request-shutdown --wait` and
//! `watchdog` (Workflow #374).

use crate::codes::ActingctlCode;
use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{IoOp, Outcome, RawSource};
use std::path::PathBuf;
use std::process::Command;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// No liveness helper exists in actingctl's dependencies and unsafe code is forbidden here, so
/// this asks the platform process lister: `%SystemRoot%\System32\tasklist.exe` without a
/// window on Windows (exit 0 either way; a match is a CSV row whose second field is the pid),
/// POSIX `ps` elsewhere (no match: non-zero, no output).
///
/// A lister that did not run, or that failed, is `process_probe_failed` with the `pid`, caused
/// by the OS error, or by `foreign_other_error` with the lister's stderr and exit code
/// (Workflow #378 A5).
pub(crate) fn pid_alive(pid: u32) -> Result<bool, Outcome> {
    let pid_text = pid.to_string();
    #[cfg(windows)]
    let (program, output) = {
        use std::os::windows::process::CommandExt;
        let system_root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
        let program = PathBuf::from(system_root)
            .join("System32")
            .join("tasklist.exe");
        let output = Command::new(&program)
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .creation_flags(CREATE_NO_WINDOW)
            .output();
        (program, output)
    };
    #[cfg(not(windows))]
    let (program, output) = (
        PathBuf::from("ps"),
        Command::new("ps")
            .args(["-p", &pid_text, "-o", "pid="])
            .output(),
    );
    let failed = || Outcome::new(ActingctlCode::ProcessProbeFailed).pid(pid);
    let output = output.map_err(|error| {
        failed().caused_by(Outcome::from_io_error(&error, IoOp::Spawn).path(&program))
    })?;
    let listed = String::from_utf8_lossy(&output.stdout).lines().any(|line| {
        line.split(',')
            .take(2)
            .any(|field| field.trim().trim_matches('"') == pid_text)
    });
    if listed {
        return Ok(true);
    }
    if output.status.success() || output.stderr.is_empty() {
        return Ok(false);
    }
    let lister = Outcome::new(ContractCode::ForeignOtherError).raw_source(RawSource::Process);
    let lister = match output.status.code() {
        Some(exit_code) => lister.exit_code(exit_code),
        None => lister,
    };
    Err(failed().caused_by(lister.raw_text(&output.stderr)))
}
