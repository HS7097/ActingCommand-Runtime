// SPDX-License-Identifier: AGPL-3.0-only

use crate::adb::{Adb, AdbConfig, stop_child};
use crate::{DeviceError, DeviceInfo, DeviceResult, DeviceTarget, HandshakeInfo, InputBackend};
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
const MAX_GESTURE_MS: u64 = 60_000;

#[derive(Debug, Clone)]
pub struct MinitouchConfig {
    pub local_path: PathBuf,
    pub remote_path: String,
    pub push: bool,
    pub handshake_timeout: Duration,
    pub shutdown_timeout: Duration,
    pub default_pressure: i32,
    pub tap_hold: Duration,
}

impl Default for MinitouchConfig {
    fn default() -> Self {
        Self {
            local_path: default_minitouch_local_path(),
            remote_path: "/data/local/tmp/minitouch".to_string(),
            push: true,
            handshake_timeout: Duration::from_secs(8),
            shutdown_timeout: Duration::from_secs(1),
            default_pressure: DEFAULT_PRESSURE,
            tap_hold: Duration::from_millis(TAP_HOLD_MS),
        }
    }
}

fn default_minitouch_local_path() -> PathBuf {
    std::env::var_os("ACTINGCOMMAND_MINITOUCH_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from("external-tools")
                .join("minitouch")
                .join("minitouch")
        })
}

pub struct MinitouchBackend {
    adb_config: AdbConfig,
    target: DeviceTarget,
    minitouch_config: MinitouchConfig,
    serial: String,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    handshake_info: Option<HandshakeInfo>,
    coordinate_mapper: Option<MinitouchCoordinateMapper>,
    stderr_text: Arc<Mutex<String>>,
    stderr_thread: Option<JoinHandle<()>>,
    closed: bool,
}

impl MinitouchBackend {
    pub fn new(
        adb_config: AdbConfig,
        target: DeviceTarget,
        minitouch_config: MinitouchConfig,
    ) -> Self {
        let serial = target.resolved_serial();
        Self {
            adb_config,
            target,
            minitouch_config,
            serial,
            child: None,
            stdin: None,
            handshake_info: None,
            coordinate_mapper: None,
            stderr_text: Arc::new(Mutex::new(String::new())),
            stderr_thread: None,
            closed: true,
        }
    }

    pub fn handshake_info(&self) -> Option<&HandshakeInfo> {
        self.handshake_info.as_ref()
    }

    pub fn connect(&mut self) -> DeviceResult<DeviceInfo> {
        if self.child.is_some() || self.stdin.is_some() {
            return Err(DeviceError::fatal("MinitouchBackend is already connected"));
        }

        let adb = Adb::new(self.adb_config.clone());
        let device = verify_minitouch_device(&adb, &self.serial, self.target.connect)?;
        self.install(&adb)?;
        self.start(&device)?;
        Ok(device)
    }

    pub fn install(&self, adb: &Adb) -> DeviceResult<()> {
        if self.minitouch_config.push {
            require_minitouch_file(&self.minitouch_config.local_path)?;
            let local = self
                .minitouch_config
                .local_path
                .to_string_lossy()
                .to_string();
            adb.push(&self.serial, &local, &self.minitouch_config.remote_path)
                .map_err(|err| {
                    DeviceError::transient(format!("failed to push minitouch binary: {err}"))
                })?;
        }

        adb.chmod(&self.serial, &self.minitouch_config.remote_path, "755")
            .map_err(|err| {
                DeviceError::transient(format!("failed to chmod minitouch binary: {err}"))
            })?;
        Ok(())
    }

    pub fn start(&mut self, device: &DeviceInfo) -> DeviceResult<()> {
        if self.child.is_some() || self.stdin.is_some() {
            return Err(DeviceError::fatal("minitouch process is already started"));
        }

        let mut child = Command::new(&self.adb_config.adb_path)
            .args([
                "-s",
                &self.serial,
                "shell",
                &self.minitouch_config.remote_path,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| {
                DeviceError::transient(format!("failed to start minitouch process: {err}"))
            })?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| DeviceError::transient("failed to open minitouch stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| DeviceError::transient("failed to open minitouch stderr"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| DeviceError::transient("failed to open minitouch stdin"))?;

        self.stderr_text = Arc::new(Mutex::new(String::new()));
        self.stderr_thread = Some(spawn_minitouch_stderr_reader(
            stderr,
            Arc::clone(&self.stderr_text),
        ));

        let handshake = match self.read_handshake(stdout, &mut child) {
            Ok(handshake) => handshake,
            Err(err) => {
                stop_child(&mut child, self.minitouch_config.shutdown_timeout);
                let join_result = self.join_stderr_thread();
                return combine_operation_and_close(Err(err), join_result);
            }
        };
        if let Err(err) = validate_default_pressure(
            self.minitouch_config.default_pressure,
            handshake.max_pressure,
        ) {
            stop_child(&mut child, self.minitouch_config.shutdown_timeout);
            let join_result = self.join_stderr_thread();
            return combine_operation_and_close(Err(self.with_stderr(err)), join_result);
        }
        let screen_bounds = match screen_bounds_from_device(device) {
            Ok(bounds) => bounds,
            Err(err) => {
                stop_child(&mut child, self.minitouch_config.shutdown_timeout);
                let join_result = self.join_stderr_thread();
                return combine_operation_and_close(Err(self.with_stderr(err)), join_result);
            }
        };
        self.coordinate_mapper = Some(MinitouchCoordinateMapper::new(screen_bounds, &handshake));
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
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let _ = tx.send(parse_minitouch_handshake(&mut reader));
        });

        match rx.recv_timeout(self.minitouch_config.handshake_timeout) {
            Ok(result) => result.map_err(|err| self.with_stderr(err)),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                stop_child(child, self.minitouch_config.shutdown_timeout);
                Err(self.with_stderr(DeviceError::transient(format!(
                    "timed out after {:?} waiting for minitouch handshake",
                    self.minitouch_config.handshake_timeout
                ))))
            }
            Err(err) => {
                stop_child(child, self.minitouch_config.shutdown_timeout);
                Err(self.with_stderr(DeviceError::transient(format!(
                    "failed to receive minitouch handshake: {err}"
                ))))
            }
        }
    }

    fn ensure_active(&mut self) -> DeviceResult<()> {
        if self.closed || self.child.is_none() || self.stdin.is_none() {
            return Err(DeviceError::fatal("MinitouchBackend is not connected"));
        }

        let child = self
            .child
            .as_mut()
            .ok_or_else(|| DeviceError::transient("minitouch child process is missing"))?;
        if let Some(status) = child.try_wait().map_err(|err| {
            DeviceError::transient(format!("failed to poll minitouch process status: {err}"))
        })? {
            return Err(self.with_stderr(DeviceError::transient(format!(
                "minitouch process exited unexpectedly with {status}"
            ))));
        }

        Ok(())
    }

    fn map_and_validate(&self, label: &str, x: i32, y: i32) -> DeviceResult<(i32, i32)> {
        if x < 0 || y < 0 {
            return Err(DeviceError::fatal(format!(
                "minitouch {label} coordinate must be non-negative: x={x}, y={y}"
            )));
        }
        let mapper = self
            .coordinate_mapper
            .as_ref()
            .ok_or_else(|| DeviceError::fatal("minitouch coordinate mapper is missing"))?;
        mapper.map(label, x, y)
    }

    fn validate_pressure(&self, pressure: i32) -> DeviceResult<()> {
        let handshake = self
            .handshake_info
            .as_ref()
            .ok_or_else(|| DeviceError::fatal("minitouch handshake info is missing"))?;
        if pressure <= 0 {
            return Err(DeviceError::fatal(format!(
                "minitouch pressure must be positive: {pressure}"
            )));
        }
        if pressure > handshake.max_pressure {
            return Err(DeviceError::fatal(format!(
                "minitouch pressure {pressure} exceeds max_pressure {}",
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
            .ok_or_else(|| DeviceError::transient("minitouch stdin is missing"))?;
        stdin.write_all(commands.as_bytes()).map_err(|err| {
            DeviceError::transient(format!("failed to write minitouch command: {err}"))
        })?;
        stdin.flush().map_err(|err| {
            DeviceError::transient(format!("failed to flush minitouch command: {err}"))
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
            DeviceError::with_severity(
                err.severity(),
                format!("{}\nminitouch stderr:\n{stderr}", err.message()),
            )
        }
    }

    fn join_stderr_thread(&mut self) -> DeviceResult<()> {
        if let Some(thread) = self.stderr_thread.take() {
            thread.join().map_err(|_| {
                DeviceError::fatal("minitouch stderr reader thread panicked during shutdown")
            })?;
        }
        Ok(())
    }

    fn shutdown(&mut self) -> Vec<DeviceError> {
        let mut errors = Vec::new();
        self.stdin.take();

        if let Some(mut child) = self.child.take()
            && !stop_child(&mut child, self.minitouch_config.shutdown_timeout)
        {
            errors.push(DeviceError::fatal(format!(
                "minitouch process did not exit within {:?}",
                self.minitouch_config.shutdown_timeout
            )));
        }

        if let Err(err) = self.join_stderr_thread() {
            errors.push(err);
        }

        self.closed = true;
        errors
    }
}

impl InputBackend for MinitouchBackend {
    fn tap(&mut self, x: i32, y: i32) -> DeviceResult<()> {
        self.long_tap(x, y, self.minitouch_config.tap_hold.as_millis() as u64)
    }

    fn long_tap(&mut self, x: i32, y: i32, duration_ms: u64) -> DeviceResult<()> {
        let duration_ms = bounded_gesture_duration_ms(duration_ms);
        let pressure = self.minitouch_config.default_pressure;
        self.validate_pressure(pressure)?;
        let (touch_x, touch_y) = self.map_and_validate("long_tap", x, y)?;
        self.write_and_flush(&format!("d {TOUCH_ID} {touch_x} {touch_y} {pressure}\nc\n"))?;
        thread::sleep(Duration::from_millis(duration_ms));
        self.write_and_flush(&format!("u {TOUCH_ID}\nc\n"))?;
        Ok(())
    }

    fn swipe(&mut self, x1: i32, y1: i32, x2: i32, y2: i32, duration_ms: u64) -> DeviceResult<()> {
        let duration_ms = bounded_gesture_duration_ms(duration_ms);
        let pressure = self.minitouch_config.default_pressure;
        self.validate_pressure(pressure)?;
        let (start_x, start_y) = self.map_and_validate("swipe start", x1, y1)?;
        let (end_x, end_y) = self.map_and_validate("swipe end", x2, y2)?;
        self.write_and_flush(&format!("d {TOUCH_ID} {start_x} {start_y} {pressure}\nc\n"))?;

        let points = swipe_points(x1, y1, x2, y2, duration_ms);
        let delay = swipe_step_delay(duration_ms, points.len());
        for (x, y) in points {
            let (touch_x, touch_y) = self.map_and_validate("swipe move", x, y)?;
            self.write_and_flush(&format!("m {TOUCH_ID} {touch_x} {touch_y} {pressure}\nc\n"))?;
            if !delay.is_zero() {
                thread::sleep(delay);
            }
        }

        self.write_and_flush(&format!(
            "m {TOUCH_ID} {end_x} {end_y} {pressure}\nc\nu {TOUCH_ID}\nc\n"
        ))?;
        Ok(())
    }

    fn key(&mut self, _key: &str) -> DeviceResult<()> {
        Err(DeviceError::fatal(
            "MinitouchBackend key input is outside A1.1 touch fallback scope",
        ))
    }

    fn text(&mut self, _text: &str) -> DeviceResult<()> {
        Err(DeviceError::fatal(
            "MinitouchBackend text input is outside A1.1 touch fallback scope",
        ))
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
            errors.push(DeviceError::fatal(format!("minitouch stderr:\n{stderr}")));
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

impl Drop for MinitouchBackend {
    fn drop(&mut self) {
        if !self.closed {
            let _ = self.shutdown();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ScreenBounds {
    width: i32,
    height: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MinitouchCoordinateMapper {
    screen: ScreenBounds,
    max_x: i32,
    max_y: i32,
    orientation: MinitouchOrientation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MinitouchOrientation {
    Same,
    LandscapeFromPortraitRaw,
    PortraitFromLandscapeRaw,
}

impl MinitouchCoordinateMapper {
    fn new(screen: ScreenBounds, handshake: &HandshakeInfo) -> Self {
        let orientation = if screen.width > screen.height && handshake.max_x < handshake.max_y {
            MinitouchOrientation::LandscapeFromPortraitRaw
        } else if screen.width < screen.height && handshake.max_x > handshake.max_y {
            MinitouchOrientation::PortraitFromLandscapeRaw
        } else {
            MinitouchOrientation::Same
        };
        Self {
            screen,
            max_x: handshake.max_x,
            max_y: handshake.max_y,
            orientation,
        }
    }

    fn map(&self, label: &str, x: i32, y: i32) -> DeviceResult<(i32, i32)> {
        if x >= self.screen.width || y >= self.screen.height {
            return Err(DeviceError::fatal(format!(
                "minitouch {label} coordinate {x},{y} exceeds screen bounds {}x{}",
                self.screen.width, self.screen.height
            )));
        }

        let mapped = match self.orientation {
            MinitouchOrientation::Same => (
                scale_coordinate(x, self.screen.width, self.max_x),
                scale_coordinate(y, self.screen.height, self.max_y),
            ),
            MinitouchOrientation::LandscapeFromPortraitRaw => (
                scale_coordinate(y, self.screen.height, self.max_x),
                scale_coordinate(self.screen.width - 1 - x, self.screen.width, self.max_y),
            ),
            MinitouchOrientation::PortraitFromLandscapeRaw => (
                scale_coordinate(self.screen.height - 1 - y, self.screen.height, self.max_x),
                scale_coordinate(x, self.screen.width, self.max_y),
            ),
        };
        validate_mapped_coordinate(label, mapped.0, self.max_x)?;
        validate_mapped_coordinate(label, mapped.1, self.max_y)?;
        Ok(mapped)
    }
}

fn scale_coordinate(value: i32, source_extent: i32, target_max: i32) -> i32 {
    if source_extent <= 1 {
        return 0;
    }
    ((value as i64 * target_max as i64) / (source_extent as i64 - 1)) as i32
}

fn validate_mapped_coordinate(label: &str, value: i32, max: i32) -> DeviceResult<()> {
    if value < 0 || value > max {
        return Err(DeviceError::fatal(format!(
            "minitouch {label} mapped coordinate {value} is outside 0..={max}"
        )));
    }
    Ok(())
}

fn parse_minitouch_handshake(reader: &mut dyn BufRead) -> DeviceResult<HandshakeInfo> {
    let mut version = String::new();
    let mut max = String::new();
    let mut pid = String::new();
    reader.read_line(&mut version).map_err(|err| {
        DeviceError::transient(format!("failed to read minitouch version: {err}"))
    })?;
    reader.read_line(&mut max).map_err(|err| {
        DeviceError::transient(format!("failed to read minitouch max line: {err}"))
    })?;
    reader
        .read_line(&mut pid)
        .map_err(|err| DeviceError::transient(format!("failed to read minitouch pid: {err}")))?;

    if !version.starts_with('v') {
        return Err(DeviceError::transient(format!(
            "invalid minitouch version line: {version:?}"
        )));
    }
    let max_values = max
        .strip_prefix('^')
        .ok_or_else(|| DeviceError::transient(format!("invalid minitouch max line: {max:?}")))?;
    let parts = max_values.split_whitespace().collect::<Vec<_>>();
    if parts.len() != 4 {
        return Err(DeviceError::transient(format!(
            "invalid minitouch max line field count: {max:?}"
        )));
    }

    let parse_i32 = |label: &str, value: &str| {
        value.parse::<i32>().map_err(|err| {
            DeviceError::transient(format!("invalid minitouch {label} '{value}': {err}"))
        })
    };
    let max_contacts = parse_i32("max_contacts", parts[0])?;
    let max_x = parse_i32("max_x", parts[1])?;
    let max_y = parse_i32("max_y", parts[2])?;
    let max_pressure = parse_i32("max_pressure", parts[3])?;
    if max_contacts <= 0 || max_x <= 0 || max_y <= 0 || max_pressure <= 0 {
        return Err(DeviceError::transient(format!(
            "minitouch handshake values must be positive: {max:?}"
        )));
    }
    let pid = pid
        .strip_prefix('$')
        .ok_or_else(|| DeviceError::transient(format!("invalid minitouch pid line: {pid:?}")))?
        .trim()
        .to_string();
    if pid.is_empty() {
        return Err(DeviceError::transient("minitouch pid line is empty"));
    }

    Ok(HandshakeInfo {
        max_contacts,
        max_x,
        max_y,
        max_pressure,
        pid,
    })
}

fn verify_minitouch_device(
    adb: &Adb,
    serial: &str,
    connect_allowed: bool,
) -> DeviceResult<DeviceInfo> {
    let state = adb.ensure_device(serial, connect_allowed).map_err(|err| {
        DeviceError::transient(format!("target device {serial} is not available: {err}"))
    })?;
    if state != "device" {
        return Err(DeviceError::transient(format!(
            "target device {serial} is not in device state: {state:?}"
        )));
    }
    let screen_size = adb.screen_size(serial).map_err(|err| {
        DeviceError::transient(format!("failed to read screen size for {serial}: {err}"))
    })?;
    Ok(DeviceInfo {
        serial: serial.to_string(),
        state,
        screen_size,
    })
}

fn screen_bounds_from_device(device: &DeviceInfo) -> DeviceResult<ScreenBounds> {
    let (_, dimensions) = device
        .screen_size
        .rsplit_once(':')
        .unwrap_or(("", &device.screen_size));
    let (width, height) = dimensions.trim().split_once('x').ok_or_else(|| {
        DeviceError::fatal(format!(
            "failed to parse minitouch screen bounds from adb wm size output: {}",
            device.screen_size
        ))
    })?;
    let width = width.trim().parse::<i32>().map_err(|err| {
        DeviceError::fatal(format!(
            "invalid minitouch screen width '{width}' in adb wm size output: {err}"
        ))
    })?;
    let height = height.trim().parse::<i32>().map_err(|err| {
        DeviceError::fatal(format!(
            "invalid minitouch screen height '{height}' in adb wm size output: {err}"
        ))
    })?;
    if width <= 0 || height <= 0 {
        return Err(DeviceError::fatal(format!(
            "minitouch screen bounds must be positive, got {width}x{height}"
        )));
    }
    Ok(ScreenBounds { width, height })
}

fn require_minitouch_file(path: &PathBuf) -> DeviceResult<()> {
    let metadata = fs::metadata(path).map_err(|err| {
        DeviceError::transient(format!(
            "minitouch local binary is unavailable at {}: {err}",
            path.display()
        ))
    })?;
    if !metadata.is_file() {
        return Err(DeviceError::transient(format!(
            "minitouch local path is not a file: {}",
            path.display()
        )));
    }
    Ok(())
}

fn validate_default_pressure(default_pressure: i32, max_pressure: i32) -> DeviceResult<()> {
    if default_pressure <= 0 || default_pressure > max_pressure {
        return Err(DeviceError::fatal(format!(
            "minitouch default pressure {default_pressure} is outside device pressure range 1..={max_pressure}; adjust MinitouchConfig.default_pressure"
        )));
    }
    Ok(())
}

fn bounded_gesture_duration_ms(duration_ms: u64) -> u64 {
    duration_ms.clamp(1, MAX_GESTURE_MS)
}

fn swipe_points(x1: i32, y1: i32, x2: i32, y2: i32, duration_ms: u64) -> Vec<(i32, i32)> {
    let steps = (duration_ms / SWIPE_FRAME_MS).clamp(1, MAX_SWIPE_STEPS);
    if steps <= 1 {
        return Vec::new();
    }
    (1..steps)
        .map(|step| {
            let x = x1 as i64 + ((x2 - x1) as i64 * step as i64 / steps as i64);
            let y = y1 as i64 + ((y2 - y1) as i64 * step as i64 / steps as i64);
            (x as i32, y as i32)
        })
        .collect()
}

fn swipe_step_delay(duration_ms: u64, point_count: usize) -> Duration {
    if point_count == 0 {
        return Duration::ZERO;
    }
    Duration::from_millis((duration_ms / (point_count as u64 + 1)).max(1))
}

fn spawn_minitouch_stderr_reader<R: Read + Send + 'static>(
    mut stderr: R,
    target: Arc<Mutex<String>>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut text = String::new();
        if let Err(err) = stderr.read_to_string(&mut text) {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&format!("minitouch stderr reader error: {err}"));
        }
        if let Ok(mut guard) = target.lock() {
            *guard = text;
        }
    })
}

fn combine_operation_and_close(
    operation: DeviceResult<()>,
    close: DeviceResult<()>,
) -> DeviceResult<()> {
    match (operation, close) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(err), Ok(())) | (Ok(()), Err(err)) => Err(err),
        (Err(operation_err), Err(close_err)) => Err(DeviceError::fatal(format!(
            "{operation_err}; close failed: {close_err}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minitouch_handshake() {
        let input = b"v 1\n^ 10 1279 719 255\n$ 1234\n";
        let mut reader = BufReader::new(&input[..]);
        let handshake = parse_minitouch_handshake(&mut reader).expect("handshake");

        assert_eq!(handshake.max_contacts, 10);
        assert_eq!(handshake.max_x, 1279);
        assert_eq!(handshake.max_y, 719);
        assert_eq!(handshake.max_pressure, 255);
        assert_eq!(handshake.pid, "1234");
    }

    #[test]
    fn minitouch_mapper_scales_same_orientation() {
        let mapper = MinitouchCoordinateMapper::new(
            ScreenBounds {
                width: 1280,
                height: 720,
            },
            &HandshakeInfo {
                max_contacts: 10,
                max_x: 2559,
                max_y: 1439,
                max_pressure: 255,
                pid: "1".to_string(),
            },
        );

        assert_eq!(mapper.map("tap", 1279, 719).expect("mapped"), (2559, 1439));
    }

    #[test]
    fn minitouch_mapper_rotates_landscape_from_portrait_raw() {
        let mapper = MinitouchCoordinateMapper::new(
            ScreenBounds {
                width: 1280,
                height: 720,
            },
            &HandshakeInfo {
                max_contacts: 10,
                max_x: 719,
                max_y: 1279,
                max_pressure: 255,
                pid: "1".to_string(),
            },
        );

        assert_eq!(mapper.map("tap", 1279, 0).expect("mapped"), (0, 0));
        assert_eq!(mapper.map("tap", 0, 719).expect("mapped"), (719, 1279));
    }
}
