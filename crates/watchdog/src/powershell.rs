// SPDX-License-Identifier: AGPL-3.0-only

//! Windows PowerShell for the two things actingctl cannot do without unsafe code: the CIM
//! process query (review H2) and the WMI start (X3). The fixed script travels as
//! `-EncodedCommand`; paths travel in environment variables only, never in script text.

use super::process_failure;
use crate::codes::ActingctlCode;
use actingcommand_contract::outcome::{CauseRelation, IoOp, Outcome};
use std::io::{self, Read};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub(crate) const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn powershell_exe() -> PathBuf {
    let system_root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    PathBuf::from(system_root).join(r"System32\WindowsPowerShell\v1.0\powershell.exe")
}

/// Runs `script` hidden and returns its stdout. Every failure is `failed` (the caller's code):
/// caused by the OS error when PowerShell did not run or its output could not be read, with
/// `timeout_ms` when it did not finish in time (it is killed), and caused by
/// `foreign_other_error` with its stderr and exit code when it exited non-zero.
pub(crate) fn run(
    script: &str,
    environment: &[(&str, &std::ffi::OsStr)],
    timeout: Duration,
    failed: ActingctlCode,
) -> Result<String, Outcome> {
    let program = powershell_exe();
    let mut command = Command::new(&program);
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-EncodedCommand",
            &encode_utf16_base64(script),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW);
    for (name, value) in environment {
        command.env(name, value);
    }
    let os_failure = |error: &io::Error, op: IoOp| {
        Outcome::new(failed).caused_by(Outcome::from_io_error(error, op).path(&program))
    };
    let mut child = command
        .spawn()
        .map_err(|error| os_failure(&error, IoOp::Spawn))?;
    let out = drain(child.stdout.take());
    let err = drain(child.stderr.take());
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                let timed_out = Outcome::new(failed).timeout_ms(timeout);
                return Err(match child.kill().and_then(|()| child.wait()) {
                    Ok(_) => timed_out,
                    Err(error) => timed_out.link(
                        CauseRelation::Cleanup,
                        Outcome::from_io_error(&error, IoOp::Wait).path(&program),
                    ),
                });
            }
            Err(error) => return Err(os_failure(&error, IoOp::Wait)),
        }
    };
    let stdout = collect(out).map_err(|error| os_failure(&error, IoOp::Read))?;
    let stderr = collect(err).map_err(|error| os_failure(&error, IoOp::Read))?;
    // Scripts run with `$ErrorActionPreference = 'Stop'`, so every error exits non-zero;
    // stderr alone may hold progress records and is reported only with a failure.
    if !status.success() {
        return Err(Outcome::new(failed).caused_by(process_failure(status, &stderr)));
    }
    Ok(String::from_utf8_lossy(&stdout).into_owned())
}

/// Reads one pipe to its end on its own thread, so neither pipe can fill and stall the child.
fn drain<R: Read + Send + 'static>(source: Option<R>) -> JoinHandle<io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut source) = source {
            source.read_to_end(&mut bytes)?;
        }
        Ok(bytes)
    })
}

/// A reader's bytes; a reader that panicked passes its panic on.
fn collect(reader: JoinHandle<io::Result<Vec<u8>>>) -> io::Result<Vec<u8>> {
    match reader.join() {
        Ok(read) => read,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

/// `-EncodedCommand` takes Base64 of the UTF-16LE script.
fn encode_utf16_base64(script: &str) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = script
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let triple = (u32::from(chunk[0]) << 16)
            | (u32::from(chunk.get(1).copied().unwrap_or(0)) << 8)
            | u32::from(chunk.get(2).copied().unwrap_or(0));
        for index in 0..4 {
            if index <= chunk.len() {
                encoded.push(char::from(
                    ALPHABET[((triple >> (18 - 6 * index)) & 0x3f) as usize],
                ));
            } else {
                encoded.push('=');
            }
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_are_base64_of_utf16_le() {
        // "ab" is 61 00 62 00; "abc" is 61 00 62 00 63 00.
        assert_eq!(encode_utf16_base64("ab"), "YQBiAA==");
        assert_eq!(encode_utf16_base64("abc"), "YQBiAGMA");
        assert_eq!(encode_utf16_base64(""), "");
    }
}
