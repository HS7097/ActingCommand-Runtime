// SPDX-License-Identifier: AGPL-3.0-only

use crate::adb::{Adb, AdbConfig, stop_child};
use crate::{DeviceError, DeviceResult, InputBackend};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const TOUCH_ID: i32 = 0;
const DEFAULT_PRESSURE: i32 = 50;
const TAP_HOLD_MS: u64 = 80;
const SWIPE_FRAME_MS: u64 = 16;
const MAX_SWIPE_STEPS: u64 = 60;

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
    pub default_pressure: i32,
    pub tap_hold: Duration,
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
            default_pressure: DEFAULT_PRESSURE,
            tap_hold: Duration::from_millis(TAP_HOLD_MS),
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

pub struct MaaTouchBackend {
    adb_config: AdbConfig,
    target: DeviceTarget,
    maatouch_config: MaaTouchConfig,
    serial: String,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    handshake_info: Option<HandshakeInfo>,
    stderr_text: Arc<Mutex<String>>,
    stderr_thread: Option<JoinHandle<()>>,
    closed: bool,
}

impl MaaTouchBackend {
    pub fn new(
        adb_config: AdbConfig,
        target: DeviceTarget,
        maatouch_config: MaaTouchConfig,
    ) -> Self {
        let serial = target.resolved_serial();
        Self {
            adb_config,
            target,
            maatouch_config,
            serial,
            child: None,
            stdin: None,
            handshake_info: None,
            stderr_text: Arc::new(Mutex::new(String::new())),
            stderr_thread: None,
            closed: true,
        }
    }

    pub fn serial(&self) -> &str {
        &self.serial
    }

    pub fn handshake_info(&self) -> Option<&HandshakeInfo> {
        self.handshake_info.as_ref()
    }

    pub fn connect(&mut self) -> DeviceResult<DeviceInfo> {
        if self.child.is_some() || self.stdin.is_some() {
            return Err(DeviceError::fatal("MaaTouchBackend is already connected"));
        }

        let adb = Adb::new(self.adb_config.clone());
        if self.target.connect {
            adb.connect(&self.serial)?;
        }

        let device = verify_device(&adb, &self.serial)?;
        self.install(&adb)?;
        self.start()?;
        Ok(device)
    }

    pub fn install(&self, adb: &Adb) -> DeviceResult<()> {
        if self.maatouch_config.push {
            require_file(&self.maatouch_config.local_path)?;
            let local = self
                .maatouch_config
                .local_path
                .to_string_lossy()
                .to_string();
            adb.push(&self.serial, &local, &self.maatouch_config.remote_path)?;
        }

        adb.chmod(&self.serial, &self.maatouch_config.remote_path, "755")?;
        Ok(())
    }

    pub fn start(&mut self) -> DeviceResult<()> {
        if self.child.is_some() || self.stdin.is_some() {
            return Err(DeviceError::fatal("MaaTouch process is already started"));
        }

        let mut child = Command::new(&self.adb_config.adb_path)
            .args([
                "-s",
                &self.serial,
                "shell",
                &format!("CLASSPATH={}", self.maatouch_config.remote_path),
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
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| DeviceError::fatal("failed to open MaaTouch stdin"))?;

        self.stderr_text = Arc::new(Mutex::new(String::new()));
        self.stderr_thread = Some(spawn_stderr_reader(stderr, Arc::clone(&self.stderr_text)));

        let handshake = match self.read_handshake(stdout, &mut child) {
            Ok(handshake) => handshake,
            Err(err) => {
                stop_child(&mut child, self.maatouch_config.shutdown_timeout);
                let join_result = self.join_stderr_thread();
                return combine_operation_and_close(Err(err), join_result);
            }
        };
        if let Err(err) = validate_default_pressure(
            self.maatouch_config.default_pressure,
            handshake.max_pressure,
        ) {
            stop_child(&mut child, self.maatouch_config.shutdown_timeout);
            let join_result = self.join_stderr_thread();
            return combine_operation_and_close(Err(self.with_stderr(err)), join_result);
        }
        self.child = Some(child);
        self.stdin = Some(stdin);
        self.handshake_info = Some(handshake);
        self.closed = false;
        Ok(())
    }

    pub fn read_handshake<R: Read + Send + 'static>(
        &self,
        stdout: R,
        child: &mut Child,
    ) -> DeviceResult<HandshakeInfo> {
        // MaaTouch/minitouch emits its startup handshake on stdout. After that
        // this backend only writes commands and does not expect stdout replies,
        // so letting the handshake reader consume and close stdout is safe.
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let _ = tx.send(parse_handshake(&mut reader));
        });

        match rx.recv_timeout(self.maatouch_config.handshake_timeout) {
            Ok(result) => result.map_err(|err| self.with_stderr(err)),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                stop_child(child, self.maatouch_config.shutdown_timeout);
                Err(self.with_stderr(DeviceError::fatal(format!(
                    "timed out after {:?} waiting for MaaTouch handshake",
                    self.maatouch_config.handshake_timeout
                ))))
            }
            Err(err) => {
                stop_child(child, self.maatouch_config.shutdown_timeout);
                Err(self.with_stderr(DeviceError::fatal(format!(
                    "failed to receive MaaTouch handshake: {err}"
                ))))
            }
        }
    }

    fn ensure_active(&mut self) -> DeviceResult<()> {
        if self.closed || self.child.is_none() || self.stdin.is_none() {
            return Err(DeviceError::fatal("MaaTouchBackend is not connected"));
        }

        let child = self
            .child
            .as_mut()
            .ok_or_else(|| DeviceError::fatal("MaaTouch child process is missing"))?;
        if let Some(status) = child.try_wait().map_err(|err| {
            DeviceError::fatal(format!("failed to poll MaaTouch process status: {err}"))
        })? {
            return Err(self.with_stderr(DeviceError::fatal(format!(
                "MaaTouch process exited unexpectedly with {status}"
            ))));
        }

        Ok(())
    }

    fn validate_input(&self, x: i32, y: i32, pressure: i32) -> DeviceResult<()> {
        let handshake = self
            .handshake_info
            .as_ref()
            .ok_or_else(|| DeviceError::fatal("MaaTouch handshake info is missing"))?;

        if x < 0 || y < 0 {
            return Err(DeviceError::fatal(format!(
                "MaaTouch coordinate must be non-negative: x={x}, y={y}"
            )));
        }
        if x > handshake.max_x {
            return Err(DeviceError::fatal(format!(
                "MaaTouch x coordinate {x} exceeds max_x {}",
                handshake.max_x
            )));
        }
        if y > handshake.max_y {
            return Err(DeviceError::fatal(format!(
                "MaaTouch y coordinate {y} exceeds max_y {}",
                handshake.max_y
            )));
        }
        if pressure <= 0 {
            return Err(DeviceError::fatal(format!(
                "MaaTouch pressure must be positive: {pressure}"
            )));
        }
        if pressure > handshake.max_pressure {
            return Err(DeviceError::fatal(format!(
                "MaaTouch pressure {pressure} exceeds max_pressure {}",
                handshake.max_pressure
            )));
        }
        Ok(())
    }

    fn write_and_flush(&mut self, commands: &str) -> DeviceResult<()> {
        self.ensure_active()?;
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| DeviceError::fatal("MaaTouch stdin is missing"))?;
        stdin.write_all(commands.as_bytes()).map_err(|err| {
            DeviceError::fatal(format!("failed to write MaaTouch command: {err}"))
        })?;
        stdin.flush().map_err(|err| {
            DeviceError::fatal(format!("failed to flush MaaTouch command: {err}"))
        })?;
        Ok(())
    }

    fn with_stderr(&self, err: DeviceError) -> DeviceError {
        let stderr = self
            .stderr_text
            .lock()
            .map(|value| value.trim().to_string())
            .unwrap_or_default();
        if stderr.is_empty() {
            err
        } else {
            DeviceError::fatal(format!("{err}\nMaaTouch stderr:\n{stderr}"))
        }
    }

    fn join_stderr_thread(&mut self) -> DeviceResult<()> {
        if let Some(thread) = self.stderr_thread.take() {
            thread.join().map_err(|_| {
                DeviceError::fatal("MaaTouch stderr reader thread panicked during shutdown")
            })?;
        }
        Ok(())
    }

    fn shutdown(&mut self) -> Vec<DeviceError> {
        let mut errors = Vec::new();
        self.stdin.take();

        if let Some(mut child) = self.child.take()
            && !stop_child(&mut child, self.maatouch_config.shutdown_timeout)
        {
            errors.push(DeviceError::fatal(format!(
                "MaaTouch process did not exit within {:?}",
                self.maatouch_config.shutdown_timeout
            )));
        }

        if let Err(err) = self.join_stderr_thread() {
            errors.push(err);
        }

        self.closed = true;
        errors
    }
}

impl InputBackend for MaaTouchBackend {
    fn tap(&mut self, x: i32, y: i32) -> DeviceResult<()> {
        self.long_tap(x, y, self.maatouch_config.tap_hold.as_millis() as u64)
    }

    fn long_tap(&mut self, x: i32, y: i32, duration_ms: u64) -> DeviceResult<()> {
        let pressure = self.maatouch_config.default_pressure;
        self.validate_input(x, y, pressure)?;
        self.write_and_flush(&format!("d {TOUCH_ID} {x} {y} {pressure}\nc\n"))?;
        thread::sleep(Duration::from_millis(duration_ms));
        self.write_and_flush(&format!("u {TOUCH_ID}\nc\n"))?;
        Ok(())
    }

    fn swipe(&mut self, x1: i32, y1: i32, x2: i32, y2: i32, duration_ms: u64) -> DeviceResult<()> {
        let pressure = self.maatouch_config.default_pressure;
        self.validate_input(x1, y1, pressure)?;
        self.validate_input(x2, y2, pressure)?;
        self.write_and_flush(&format!("d {TOUCH_ID} {x1} {y1} {pressure}\nc\n"))?;

        let points = swipe_points(x1, y1, x2, y2, duration_ms);
        let delay = swipe_step_delay(duration_ms, points.len());
        for (x, y) in points {
            self.write_and_flush(&format!("m {TOUCH_ID} {x} {y} {pressure}\nc\n"))?;
            if !delay.is_zero() {
                thread::sleep(delay);
            }
        }

        self.write_and_flush(&format!(
            "m {TOUCH_ID} {x2} {y2} {pressure}\nc\nu {TOUCH_ID}\nc\n"
        ))?;
        Ok(())
    }

    fn reset(&mut self) -> DeviceResult<()> {
        self.write_and_flush("r\nc\n")
    }

    fn close(&mut self) -> DeviceResult<()> {
        if self.closed {
            return Ok(());
        }

        let mut errors = Vec::new();
        if let Err(err) = self.reset() {
            errors.push(err);
        }
        errors.extend(self.shutdown());

        let stderr = self
            .stderr_text
            .lock()
            .map(|value| value.trim().to_string())
            .unwrap_or_default();
        if !stderr.is_empty() && stderr != "Killed" {
            errors.push(DeviceError::fatal(format!("MaaTouch stderr:\n{stderr}")));
        }

        self.closed = true;
        if errors.is_empty() {
            Ok(())
        } else {
            Err(DeviceError::fatal(
                errors
                    .into_iter()
                    .map(|err| err.to_string())
                    .collect::<Vec<_>>()
                    .join("; "),
            ))
        }
    }
}

impl Drop for MaaTouchBackend {
    fn drop(&mut self) {
        if !self.closed {
            let _ = self.shutdown();
        }
    }
}

/// Smoke-test helper for CLI probes; production callers should use `MaaTouchBackend` directly.
pub fn validate_maatouch(
    config: &MaaTouchValidationConfig,
) -> DeviceResult<MaaTouchValidationResult> {
    let mut backend = MaaTouchBackend::new(
        config.adb.clone(),
        config.target.clone(),
        config.maatouch.clone(),
    );
    let device = backend.connect()?;
    let handshake = backend
        .handshake_info()
        .cloned()
        .ok_or_else(|| DeviceError::fatal("MaaTouch handshake was not recorded after connect"))?;

    let operation_result = run_touch_plan(&mut backend, &config.touch_plan);
    let close_result = backend.close();
    combine_operation_and_close(operation_result, close_result)?;

    Ok(MaaTouchValidationResult { device, handshake })
}

fn validate_default_pressure(default_pressure: i32, max_pressure: i32) -> DeviceResult<()> {
    if default_pressure <= 0 || default_pressure > max_pressure {
        return Err(DeviceError::fatal(format!(
            "MaaTouch default pressure {default_pressure} is outside device pressure range 1..={max_pressure}; adjust MaaTouchConfig.default_pressure"
        )));
    }
    Ok(())
}

fn run_touch_plan(backend: &mut MaaTouchBackend, touch_plan: &TouchPlan) -> DeviceResult<()> {
    backend.reset()?;
    if let Some(wake) = touch_plan.wake_first {
        backend.long_tap(
            wake.x,
            wake.y,
            backend.maatouch_config.tap_hold.as_millis() as u64,
        )?;
        thread::sleep(touch_plan.between_tap_delay);
    }
    if let Some(tap) = touch_plan.tap {
        backend.long_tap(
            tap.x,
            tap.y,
            backend.maatouch_config.tap_hold.as_millis() as u64,
        )?;
    }
    if !touch_plan.post_command_delay.is_zero() {
        thread::sleep(touch_plan.post_command_delay);
    }
    Ok(())
}

pub fn combine_operation_and_close(
    operation_result: DeviceResult<()>,
    close_result: DeviceResult<()>,
) -> DeviceResult<()> {
    match (operation_result, close_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(operation), Ok(())) => Err(operation),
        (Ok(()), Err(close)) => Err(close),
        (Err(operation), Err(close)) => Err(DeviceError::fatal(format!(
            "{operation}; additionally failed to close MaaTouch: {close}"
        ))),
    }
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

fn spawn_stderr_reader(
    stderr: impl Read + Send + 'static,
    target: Arc<Mutex<String>>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut text = String::new();
        let _ = reader.read_to_string(&mut text);
        if let Ok(mut target) = target.lock() {
            *target = text;
        }
    })
}

fn parse_handshake<R: Read>(reader: &mut BufReader<R>) -> DeviceResult<HandshakeInfo> {
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

fn swipe_points(x1: i32, y1: i32, x2: i32, y2: i32, duration_ms: u64) -> Vec<(i32, i32)> {
    let steps = (duration_ms / SWIPE_FRAME_MS).clamp(1, MAX_SWIPE_STEPS);
    (1..steps)
        .map(|step| {
            let ratio = step as f64 / steps as f64;
            let x = x1 as f64 + (x2 - x1) as f64 * ratio;
            let y = y1 as f64 + (y2 - y1) as f64 * ratio;
            (x.round() as i32, y.round() as i32)
        })
        .collect()
}

fn swipe_step_delay(duration_ms: u64, point_count: usize) -> Duration {
    if point_count == 0 || duration_ms == 0 {
        return Duration::ZERO;
    }
    Duration::from_millis((duration_ms / (point_count as u64 + 1)).max(1))
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
        let info = parse_handshake(&mut reader).expect("handshake");
        assert_eq!(info.max_contacts, 10);
        assert_eq!(info.max_x, 1280);
        assert_eq!(info.max_y, 720);
        assert_eq!(info.max_pressure, 255);
        assert_eq!(info.pid, "12345");
    }

    #[test]
    fn swipe_points_include_intermediate_points_only() {
        let points = swipe_points(0, 0, 100, 0, 64);
        assert_eq!(points, vec![(25, 0), (50, 0), (75, 0)]);
    }

    #[test]
    fn validate_input_accepts_legal_values() {
        let backend = backend_with_handshake();
        backend.validate_input(1280, 720, 255).expect("valid input");
    }

    #[test]
    fn validate_input_rejects_missing_handshake() {
        let backend = MaaTouchBackend::new(
            AdbConfig::default(),
            DeviceTarget::default(),
            MaaTouchConfig::default(),
        );
        assert_fatal(backend.validate_input(100, 100, 50));
    }

    #[test]
    fn validate_input_rejects_negative_coordinate() {
        let backend = backend_with_handshake();
        assert_fatal(backend.validate_input(-1, 100, 50));
    }

    #[test]
    fn validate_input_rejects_x_over_max() {
        let backend = backend_with_handshake();
        assert_fatal(backend.validate_input(1281, 100, 50));
    }

    #[test]
    fn validate_input_rejects_y_over_max() {
        let backend = backend_with_handshake();
        assert_fatal(backend.validate_input(100, 721, 50));
    }

    #[test]
    fn validate_input_rejects_non_positive_pressure() {
        let backend = backend_with_handshake();
        assert_fatal(backend.validate_input(100, 100, 0));
    }

    #[test]
    fn validate_input_rejects_pressure_over_max() {
        let backend = backend_with_handshake();
        assert_fatal(backend.validate_input(100, 100, 256));
    }

    #[test]
    fn validate_default_pressure_rejects_device_range_mismatch() {
        assert_fatal(validate_default_pressure(50, 49));
    }

    #[test]
    fn validation_uses_backend_style_close_error_combination() {
        let operation = Err(DeviceError::fatal("operation failed"));
        let close = Err(DeviceError::fatal("close failed"));
        let err = combine_operation_and_close(operation, close).expect_err("combined error");
        assert!(err.message().contains("operation failed"));
        assert!(err.message().contains("close failed"));
    }

    fn backend_with_handshake() -> MaaTouchBackend {
        let mut backend = MaaTouchBackend::new(
            AdbConfig::default(),
            DeviceTarget::default(),
            MaaTouchConfig::default(),
        );
        backend.handshake_info = Some(HandshakeInfo {
            max_contacts: 10,
            max_x: 1280,
            max_y: 720,
            max_pressure: 255,
            pid: "12345".to_string(),
        });
        backend
    }

    fn assert_fatal(result: DeviceResult<()>) {
        let err = result.expect_err("expected fatal device error");
        assert_eq!(err.severity(), crate::DeviceErrorSeverity::Fatal);
    }
}
