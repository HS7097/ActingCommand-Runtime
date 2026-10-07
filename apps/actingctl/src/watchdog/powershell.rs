// SPDX-License-Identifier: AGPL-3.0-only

//! Windows PowerShell for the two things actingctl cannot do without unsafe code: the CIM
//! process query (review H2) and the WMI start (X3). The fixed script travels as
//! `-EncodedCommand`; paths travel in environment variables only, never in script text.

use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub(crate) const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn powershell_exe() -> PathBuf {
    let system_root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    PathBuf::from(system_root).join(r"System32\WindowsPowerShell\v1.0\powershell.exe")
}

/// Runs `script` hidden and returns its stdout; a non-zero exit or a timeout is an error that
/// names what PowerShell printed.
pub(crate) fn run(
    script: &str,
    environment: &[(&str, &std::ffi::OsStr)],
    timeout: Duration,
) -> Result<String, String> {
    let mut command = Command::new(powershell_exe());
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
    let mut child = command
        .spawn()
        .map_err(|error| format!("powershell did not run: {error}"))?;
    let mut stdout = child.stdout.take().ok_or("powershell stdout unavailable")?;
    let mut stderr = child.stderr.take().ok_or("powershell stderr unavailable")?;
    let out = thread::spawn(move || {
        let mut text = Vec::new();
        stdout.read_to_end(&mut text).map(|_| text)
    });
    let err = thread::spawn(move || {
        let mut text = Vec::new();
        stderr.read_to_end(&mut text).map(|_| text)
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                let killed = child.kill().and_then(|()| child.wait());
                return Err(format!(
                    "powershell did not finish within {} s (kill: {killed:?})",
                    timeout.as_secs()
                ));
            }
            Err(error) => return Err(format!("powershell state unavailable: {error}")),
        }
    };
    let stdout = out
        .join()
        .map_err(|_| "powershell stdout reader panicked".to_owned())?
        .map_err(|error| format!("powershell stdout unreadable: {error}"))?;
    let stderr = err
        .join()
        .map_err(|_| "powershell stderr reader panicked".to_owned())?
        .map_err(|error| format!("powershell stderr unreadable: {error}"))?;
    // Scripts run with `$ErrorActionPreference = 'Stop'`, so every error exits non-zero;
    // stderr alone may hold progress records and is reported only with a failure.
    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr).trim().to_owned();
        return Err(format!("powershell failed ({status}): {stderr}"));
    }
    Ok(String::from_utf8_lossy(&stdout).into_owned())
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
