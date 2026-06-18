// SPDX-License-Identifier: AGPL-3.0-only

use crate::{DeviceError, DeviceResult};
use std::io::{self, Read};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct AdbConfig {
    pub adb_path: String,
    pub command_timeout: Duration,
}

impl Default for AdbConfig {
    fn default() -> Self {
        Self {
            adb_path: "adb".to_string(),
            command_timeout: Duration::from_secs(12),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone)]
pub struct Adb {
    config: AdbConfig,
}

impl Adb {
    pub fn new(config: AdbConfig) -> Self {
        Self { config }
    }

    pub fn connect(&self, serial: &str) -> DeviceResult<CommandOutput> {
        self.run(&["connect", serial])
    }

    pub fn get_state(&self, serial: &str) -> DeviceResult<String> {
        let output = self.run(&["-s", serial, "get-state"])?;
        Ok(output.stdout.trim().to_string())
    }

    pub fn screen_size(&self, serial: &str) -> DeviceResult<String> {
        let output = self.run(&["-s", serial, "shell", "wm", "size"])?;
        Ok(output.stdout.trim().to_string())
    }

    pub fn push(&self, serial: &str, local: &str, remote: &str) -> DeviceResult<CommandOutput> {
        self.run(&["-s", serial, "push", local, remote])
    }

    pub fn chmod(&self, serial: &str, remote: &str, mode: &str) -> DeviceResult<CommandOutput> {
        self.run(&["-s", serial, "shell", "chmod", mode, remote])
    }

    pub fn run(&self, args: &[&str]) -> DeviceResult<CommandOutput> {
        run_with_timeout(&self.config.adb_path, args, self.config.command_timeout)
    }
}

pub fn run_with_timeout(
    adb_path: &str,
    args: &[&str],
    timeout: Duration,
) -> DeviceResult<CommandOutput> {
    let mut child = Command::new(adb_path)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| {
            DeviceError::fatal(format!("failed to spawn adb {}: {err}", args.join(" ")))
        })?;

    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(|err| {
            DeviceError::fatal(format!(
                "failed to poll adb {} process: {err}",
                args.join(" ")
            ))
        })? {
            let stdout = read_pipe_to_string(child.stdout.take())?;
            let stderr = read_pipe_to_string(child.stderr.take())?;
            if status.success() {
                return Ok(CommandOutput { stdout, stderr });
            }
            return Err(DeviceError::fatal(format!(
                "adb {} failed with {status}\nstdout:\n{stdout}\nstderr:\n{stderr}",
                args.join(" ")
            )));
        }
        if started.elapsed() >= timeout {
            stop_child(&mut child, Duration::from_millis(500));
            let stdout = read_pipe_to_string(child.stdout.take())?;
            let stderr = read_pipe_to_string(child.stderr.take())?;
            return Err(DeviceError::fatal(format!(
                "adb {} timed out after {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
                args.join(" "),
                timeout
            )));
        }
        thread::sleep(Duration::from_millis(25));
    }
}

pub fn stop_child(child: &mut Child, timeout: Duration) -> bool {
    if matches!(child.try_wait(), Ok(Some(_))) {
        return true;
    }
    let _ = child.kill();
    let started = Instant::now();
    while started.elapsed() < timeout {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return true;
        }
        thread::sleep(Duration::from_millis(25));
    }
    eprintln!("warning: child process did not exit within {:?}", timeout);
    false
}

fn read_pipe_to_string<R: Read>(pipe: Option<R>) -> DeviceResult<String> {
    let mut text = String::new();
    if let Some(mut reader) = pipe {
        reader.read_to_string(&mut text).map_err(|err: io::Error| {
            DeviceError::fatal(format!("failed to read adb pipe: {err}"))
        })?;
    }
    Ok(text)
}
