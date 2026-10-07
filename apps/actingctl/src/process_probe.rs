// SPDX-License-Identifier: AGPL-3.0-only

//! Process liveness without unsafe code, shared by `request-shutdown --wait` and
//! `watchdog` (Workflow #374).

use std::process::Command;

/// No liveness helper exists in actingctl's dependencies and unsafe code is forbidden here, so
/// this asks the platform process lister: `tasklist` on Windows (exit 0 either way; a match is a
/// CSV row whose second field is the pid), POSIX `ps` elsewhere (no match: non-zero, no output).
pub(crate) fn pid_alive(pid: u32) -> Result<bool, String> {
    let pid_text = pid.to_string();
    #[cfg(windows)]
    let output = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output();
    #[cfg(not(windows))]
    let output = Command::new("ps")
        .args(["-p", &pid_text, "-o", "pid="])
        .output();
    let output = output.map_err(|error| format!("process lister did not run: {error}"))?;
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
    Err(format!(
        "process lister failed: status={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}
