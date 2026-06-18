// SPDX-License-Identifier: AGPL-3.0-only

use crate::adb::{Adb, AdbConfig, CommandOutput, stop_child};
use crate::{DeviceError, DeviceResult};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct DeviceTarget {
    pub serial: Option<String>,
    pub host: String,
    pub port: u16,
    pub connect: bool,
}

impl Default for DeviceTarget {
    fn default() -> Self {
        Self {
            serial: None,
            host: "127.0.0.1".to_string(),
            port: 16384,
            connect: true,
        }
    }
}

impl DeviceTarget {
    pub fn resolved_serial(&self) -> String {
        self.serial
            .clone()
            .unwrap_or_else(|| format!("{}:{}", self.host, self.port))
    }
}

#[derive(Debug, Clone)]
pub struct MaaTouchConfig {
    pub local_path: PathBuf,
    pub remote_path: String,
    pub push: bool,
    pub handshake_timeout: Duration,
    pub shutdown_timeout: Duration,
}

impl Default for MaaTouchConfig {
    fn default() -> Self {
        Self {
            local_path: PathBuf::from("external-tools")
                .join("maatouch")
                .join("maatouch"),
            remote_path: "/data/local/tmp/maatouch".to_string(),
            push: true,
            handshake_timeout: Duration::from_secs(8),
            shutdown_timeout: Duration::from_secs(1),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TouchAction {
    pub x: i32,
    pub y: i32,
    pub pressure: i32,
}

impl TouchAction {
    pub fn new(x: i32, y: i32, pressure: i32) -> Self {
        Self { x, y, pressure }
    }
}

#[derive(Debug, Clone)]
pub struct TouchPlan {
    pub wake_first: Option<TouchAction>,
    pub tap: Option<TouchAction>,
    pub between_tap_delay: Duration,
    pub post_command_delay: Duration,
}

impl Default for TouchPlan {
    fn default() -> Self {
        Self {
            wake_first: None,
            tap: None,
            between_tap_delay: Duration::from_secs(1),
            post_command_delay: Duration::from_millis(250),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct MaaTouchValidationConfig {
    pub adb: AdbConfig,
    pub target: DeviceTarget,
    pub maatouch: MaaTouchConfig,
    pub touch_plan: TouchPlan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandshakeInfo {
    pub max_contacts: i32,
    pub max_x: i32,
    pub max_y: i32,
    pub max_pressure: i32,
    pub pid: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub serial: String,
    pub state: String,
    pub screen_size: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaaTouchValidationResult {
    pub device: DeviceInfo,
    pub handshake: HandshakeInfo,
}

pub fn validate_maatouch(
    config: &MaaTouchValidationConfig,
) -> DeviceResult<MaaTouchValidationResult> {
    if config.maatouch.push {
        require_file(&config.maatouch.local_path)?;
    }

    let serial = config.target.resolved_serial();
    let adb = Adb::new(config.adb.clone());

    if config.target.connect {
        let output = adb.connect(&serial)?;
        print_command_output("adb connect", &output);
    }

    let device = verify_device(&adb, &serial)?;

    if config.maatouch.push {
        push_maatouch(&adb, &serial, &config.maatouch)?;
    }

    let handshake =
        run_maatouch_session(&config.adb, &serial, &config.maatouch, &config.touch_plan)?;
    Ok(MaaTouchValidationResult { device, handshake })
}

fn require_file(path: &PathBuf) -> DeviceResult<()> {
    let meta = fs::metadata(path).map_err(|err| {
        DeviceError::fatal(format!(
            "required MaaTouch file is unavailable at {}: {err}",
            path.display()
        ))
    })?;
    if meta.is_dir() {
        return Err(DeviceError::fatal(format!(
            "required MaaTouch path is a directory: {}",
            path.display()
        )));
    }
    Ok(())
}

fn verify_device(adb: &Adb, serial: &str) -> DeviceResult<DeviceInfo> {
    let state = adb.get_state(serial).map_err(|err| {
        let devices = adb
            .run(&["devices", "-l"])
            .map(|out| out.stdout)
            .unwrap_or_else(|list_err| format!("adb devices -l also failed: {list_err}"));
        DeviceError::fatal(format!(
            "target device {serial} is not available: {err}\nadb devices -l:\n{devices}"
        ))
    })?;
    if state != "device" {
        return Err(DeviceError::fatal(format!(
            "target device {serial} is not in device state: {state:?}"
        )));
    }

    let screen_size = adb.screen_size(serial)?;
    Ok(DeviceInfo {
        serial: serial.to_string(),
        state,
        screen_size,
    })
}

fn push_maatouch(adb: &Adb, serial: &str, config: &MaaTouchConfig) -> DeviceResult<()> {
    let local = config.local_path.to_string_lossy().to_string();
    let output = adb.push(serial, &local, &config.remote_path)?;
    print_command_output("adb push", &output);

    let output = adb.chmod(serial, &config.remote_path, "755")?;
    print_command_output("adb chmod", &output);
    Ok(())
}

fn run_maatouch_session(
    adb: &AdbConfig,
    serial: &str,
    maatouch: &MaaTouchConfig,
    touch_plan: &TouchPlan,
) -> DeviceResult<HandshakeInfo> {
    let mut child = Command::new(&adb.adb_path)
        .args([
            "-s",
            serial,
            "shell",
            &format!("CLASSPATH={}", maatouch.remote_path),
            "app_process",
            "/",
            "com.shxyke.MaaTouch.App",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| {
            DeviceError::fatal(format!("failed to start MaaTouch app_process: {err}"))
        })?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| DeviceError::fatal("failed to open MaaTouch stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| DeviceError::fatal("failed to open MaaTouch stderr"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| DeviceError::fatal("failed to open MaaTouch stdin"))?;

    let stderr_text = Arc::new(Mutex::new(String::new()));
    let stderr_copy = Arc::clone(&stderr_text);
    let stderr_thread = thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut text = String::new();
        let _ = reader.read_to_string(&mut text);
        if let Ok(mut target) = stderr_copy.lock() {
            *target = text;
        }
    });

    let stdout_reader = Arc::new(Mutex::new(BufReader::new(stdout)));
    let handshake_reader = Arc::clone(&stdout_reader);
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result = match handshake_reader.lock() {
            Ok(mut reader) => read_handshake(&mut reader),
            Err(err) => Err(DeviceError::fatal(format!(
                "failed to lock MaaTouch stdout reader: {err}"
            ))),
        };
        let _ = tx.send(result);
    });

    let info = match receive_handshake(
        rx,
        &stderr_text,
        &mut child,
        maatouch.shutdown_timeout,
        maatouch.handshake_timeout,
    ) {
        Ok(info) => info,
        Err(err) => {
            drop(stdout_reader);
            let _ = stderr_thread.join();
            return Err(err);
        }
    };

    send_reset(&mut stdin)?;
    if let Some(wake) = touch_plan.wake_first {
        send_tap(&mut stdin, wake)?;
        thread::sleep(touch_plan.between_tap_delay);
    }
    if let Some(tap) = touch_plan.tap {
        send_tap(&mut stdin, tap)?;
    }

    thread::sleep(touch_plan.post_command_delay);
    drop(stdin);
    if !stop_child(&mut child, maatouch.shutdown_timeout) {
        drop(stdout_reader);
        let _ = stderr_thread.join();
        return Err(DeviceError::fatal(format!(
            "MaaTouch process did not exit within {:?}",
            maatouch.shutdown_timeout
        )));
    }
    drop(stdout_reader);
    let _ = stderr_thread.join();

    let stderr = stderr_text
        .lock()
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    if !stderr.is_empty() && stderr != "Killed" {
        return Err(DeviceError::fatal(format!("MaaTouch stderr:\n{stderr}")));
    }

    Ok(info)
}

fn receive_handshake(
    rx: mpsc::Receiver<DeviceResult<HandshakeInfo>>,
    stderr: &Arc<Mutex<String>>,
    child: &mut std::process::Child,
    shutdown_timeout: Duration,
    handshake_timeout: Duration,
) -> DeviceResult<HandshakeInfo> {
    match rx.recv_timeout(handshake_timeout) {
        Ok(result) => result.map_err(|err| attach_stderr(err, stderr)),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            stop_child(child, shutdown_timeout);
            Err(attach_stderr(
                DeviceError::fatal(format!(
                    "timed out after {:?} waiting for MaaTouch handshake",
                    handshake_timeout
                )),
                stderr,
            ))
        }
        Err(err) => {
            stop_child(child, shutdown_timeout);
            Err(attach_stderr(
                DeviceError::fatal(format!("failed to receive MaaTouch handshake: {err}")),
                stderr,
            ))
        }
    }
}

fn read_handshake<R: Read>(reader: &mut BufReader<R>) -> DeviceResult<HandshakeInfo> {
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line).map_err(|err| {
            DeviceError::fatal(format!("failed to read MaaTouch handshake: {err}"))
        })?;
        if read == 0 {
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line == "Aborted" {
            return Err(DeviceError::fatal(
                "MaaTouch reported Aborted during startup",
            ));
        }
        if let Some(rest) = line.strip_prefix("^ ") {
            return parse_version_and_pid(rest, reader);
        }
    }
    Err(DeviceError::fatal(
        "MaaTouch stdout ended before handshake was received",
    ))
}

fn parse_version_and_pid<R: Read>(
    version: &str,
    reader: &mut BufReader<R>,
) -> DeviceResult<HandshakeInfo> {
    let values = version
        .split_whitespace()
        .map(str::parse::<i32>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| DeviceError::fatal(format!("invalid MaaTouch version value: {err}")))?;
    if values.len() != 4 {
        return Err(DeviceError::fatal(format!(
            "invalid MaaTouch version line: ^ {version}"
        )));
    }

    let mut pid_line = String::new();
    reader.read_line(&mut pid_line).map_err(|err| {
        DeviceError::fatal(format!(
            "failed to read MaaTouch pid line after version: {err}"
        ))
    })?;
    let pid_line = pid_line.trim();
    let pid = pid_line
        .strip_prefix("$ ")
        .ok_or_else(|| DeviceError::fatal(format!("unexpected MaaTouch pid line: {pid_line:?}")))?
        .trim()
        .to_string();

    Ok(HandshakeInfo {
        max_contacts: values[0],
        max_x: values[1],
        max_y: values[2],
        max_pressure: values[3],
        pid,
    })
}

fn send_reset(stdin: &mut ChildStdin) -> DeviceResult<()> {
    stdin
        .write_all(b"r\nc\n")
        .map_err(|err| DeviceError::fatal(format!("failed to send MaaTouch reset: {err}")))?;
    stdin
        .flush()
        .map_err(|err| DeviceError::fatal(format!("failed to flush MaaTouch reset: {err}")))?;
    Ok(())
}

fn send_tap(stdin: &mut ChildStdin, tap: TouchAction) -> DeviceResult<()> {
    writeln!(stdin, "d 0 {} {} {}", tap.x, tap.y, tap.pressure)
        .map_err(|err| DeviceError::fatal(format!("failed to send MaaTouch down: {err}")))?;
    writeln!(stdin, "c")
        .map_err(|err| DeviceError::fatal(format!("failed to commit MaaTouch down: {err}")))?;
    stdin
        .flush()
        .map_err(|err| DeviceError::fatal(format!("failed to flush MaaTouch down: {err}")))?;
    thread::sleep(Duration::from_millis(80));
    writeln!(stdin, "u 0")
        .map_err(|err| DeviceError::fatal(format!("failed to send MaaTouch up: {err}")))?;
    writeln!(stdin, "c")
        .map_err(|err| DeviceError::fatal(format!("failed to commit MaaTouch up: {err}")))?;
    stdin
        .flush()
        .map_err(|err| DeviceError::fatal(format!("failed to flush MaaTouch up: {err}")))?;
    Ok(())
}

fn print_command_output(label: &str, output: &CommandOutput) {
    let stdout = output.stdout.trim();
    if !stdout.is_empty() {
        println!("{label} stdout: {stdout}");
    }
    let stderr = output.stderr.trim();
    if !stderr.is_empty() {
        eprintln!("{label} stderr: {stderr}");
    }
}

fn attach_stderr(err: DeviceError, stderr: &Arc<Mutex<String>>) -> DeviceError {
    let stderr = stderr
        .lock()
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    if stderr.is_empty() {
        err
    } else {
        DeviceError::fatal(format!("{err}\nMaaTouch stderr:\n{stderr}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn target_defaults_to_host_port_serial() {
        let target = DeviceTarget::default();
        assert_eq!(target.resolved_serial(), "127.0.0.1:16384");
    }

    #[test]
    fn parses_maatouch_handshake() {
        let input = Cursor::new("^ 10 1280 720 255\n$ 12345\n");
        let mut reader = BufReader::new(input);
        let info = read_handshake(&mut reader).expect("handshake");
        assert_eq!(info.max_contacts, 10);
        assert_eq!(info.max_x, 1280);
        assert_eq!(info.max_y, 720);
        assert_eq!(info.max_pressure, 255);
        assert_eq!(info.pid, "12345");
    }
}
