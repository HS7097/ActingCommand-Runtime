// SPDX-License-Identifier: AGPL-3.0-only

use crate::{DeviceError, DeviceResult};
use std::io::{self, Read};
use std::process::ExitStatus;
use std::process::{Child, Command, Stdio};
use std::thread::{self, JoinHandle};
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryOutput {
    pub stdout: Vec<u8>,
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

    pub fn screencap(&self, serial: &str, timeout: Duration) -> DeviceResult<BinaryOutput> {
        run_binary_with_timeout(
            &self.config.adb_path,
            &["-s", serial, "exec-out", "screencap", "-p"],
            timeout,
        )
    }

    pub fn forward(&self, serial: &str, local: &str, remote: &str) -> DeviceResult<CommandOutput> {
        self.run(&["-s", serial, "forward", local, remote])
    }

    pub fn shell_spawn(&self, serial: &str, args: &[&str]) -> DeviceResult<Child> {
        Command::new(&self.config.adb_path)
            .args(["-s", serial, "shell"])
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|err| {
                DeviceError::fatal(format!(
                    "failed to spawn adb shell {}: {err}",
                    args.join(" ")
                ))
            })
    }

    pub fn push(&self, serial: &str, local: &str, remote: &str) -> DeviceResult<CommandOutput> {
        self.run(&["-s", serial, "push", local, remote])
    }

    pub fn chmod(&self, serial: &str, remote: &str, mode: &str) -> DeviceResult<CommandOutput> {
        self.run(&["-s", serial, "shell", "chmod", mode, remote])
    }

    pub fn run(&self, args: &[&str]) -> DeviceResult<CommandOutput> {
        run_text_with_timeout(&self.config.adb_path, args, self.config.command_timeout)
    }
}

struct RawCommandOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

pub fn run_text_with_timeout(
    adb_path: &str,
    args: &[&str],
    timeout: Duration,
) -> DeviceResult<CommandOutput> {
    let output = run_raw_with_timeout(adb_path, args, timeout)?;
    let stdout = decode_adb_text(output.stdout, "stdout", args)?;
    let stderr = decode_adb_text(output.stderr, "stderr", args)?;
    if output.status.success() {
        return Ok(CommandOutput { stdout, stderr });
    }
    Err(DeviceError::fatal(format!(
        "adb {} failed with {}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        args.join(" "),
        output.status
    )))
}

pub fn run_binary_with_timeout(
    adb_path: &str,
    args: &[&str],
    timeout: Duration,
) -> DeviceResult<BinaryOutput> {
    let output = run_raw_with_timeout(adb_path, args, timeout)?;
    let stderr = decode_adb_text(output.stderr, "stderr", args)?;
    if output.status.success() {
        return Ok(BinaryOutput {
            stdout: output.stdout,
            stderr,
        });
    }
    Err(DeviceError::fatal(format!(
        "adb {} failed with {}\nstdout bytes: {}\nstderr:\n{stderr}",
        args.join(" "),
        output.status,
        output.stdout.len()
    )))
}

fn run_raw_with_timeout(
    adb_path: &str,
    args: &[&str],
    timeout: Duration,
) -> DeviceResult<RawCommandOutput> {
    let mut child = Command::new(adb_path)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| {
            DeviceError::fatal(format!("failed to spawn adb {}: {err}", args.join(" ")))
        })?;

    let stdout = child.stdout.take().ok_or_else(|| {
        DeviceError::fatal(format!("failed to open adb {} stdout", args.join(" ")))
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        DeviceError::fatal(format!("failed to open adb {} stderr", args.join(" ")))
    })?;
    let stdout_thread = spawn_pipe_reader(stdout);
    let stderr_thread = spawn_pipe_reader(stderr);

    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(|err| {
            DeviceError::fatal(format!(
                "failed to poll adb {} process: {err}",
                args.join(" ")
            ))
        })? {
            return collect_raw_output(status, stdout_thread, stderr_thread);
        }
        if started.elapsed() >= timeout {
            let stopped = stop_child(&mut child, Duration::from_millis(500));
            if !stopped {
                return Err(DeviceError::fatal(format!(
                    "adb {} timed out after {:?}; adb process did not exit after kill and pipe reader threads were detached to avoid a shutdown hang",
                    args.join(" "),
                    timeout
                )));
            }
            let stdout = join_pipe_reader(stdout_thread, "stdout")?;
            let stderr = join_pipe_reader(stderr_thread, "stderr")?;
            let stdout = String::from_utf8_lossy(&stdout);
            let stderr = String::from_utf8_lossy(&stderr);
            return Err(DeviceError::fatal(format!(
                "adb {} timed out after {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
                args.join(" "),
                timeout
            )));
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn collect_raw_output(
    status: ExitStatus,
    stdout_thread: JoinHandle<io::Result<Vec<u8>>>,
    stderr_thread: JoinHandle<io::Result<Vec<u8>>>,
) -> DeviceResult<RawCommandOutput> {
    Ok(RawCommandOutput {
        status,
        stdout: join_pipe_reader(stdout_thread, "stdout")?,
        stderr: join_pipe_reader(stderr_thread, "stderr")?,
    })
}

fn spawn_pipe_reader(mut reader: impl Read + Send + 'static) -> JoinHandle<io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;
        Ok(bytes)
    })
}

fn join_pipe_reader(
    thread: JoinHandle<io::Result<Vec<u8>>>,
    stream_name: &str,
) -> DeviceResult<Vec<u8>> {
    thread
        .join()
        .map_err(|_| DeviceError::fatal(format!("adb {stream_name} reader thread panicked")))?
        .map_err(|err| DeviceError::fatal(format!("failed to read adb {stream_name}: {err}")))
}

fn decode_adb_text(bytes: Vec<u8>, stream_name: &str, args: &[&str]) -> DeviceResult<String> {
    String::from_utf8(bytes).map_err(|err| {
        DeviceError::fatal(format!(
            "adb {} produced non-UTF-8 {stream_name}: {err}",
            args.join(" ")
        ))
    })
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
    false
}
